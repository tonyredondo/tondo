//! Owned nonblocking transports and non-consuming selectable preparation.

use std::{
    io,
    net::{Shutdown, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::net::{
    TcpListener as HostListener, TcpSocket, TcpStream as HostStream,
    UdpSocket as HostDatagramSocket,
};
use tondo_stdlib::{
    io::ReadResult,
    net::{
        Datagram, NetError, NetLimits, SocketAddress, tcp_read_result, tcp_write_result,
        udp_send_result,
    },
};

use super::io_error;

pub(crate) fn host_address(address: SocketAddress) -> SocketAddr {
    SocketAddr::new(address.ip().as_ip(), address.port())
}

pub(crate) fn tondo_address(address: SocketAddr) -> Result<SocketAddress, NetError> {
    SocketAddress::create(
        tondo_stdlib::net::IpAddress::from_ip(address.ip()),
        address.port().into(),
    )
}

/// A prepared connection belongs to the listener until commit. Dropping an
/// `accept` wait or a losing select arm leaves it available to the next caller.
pub(crate) struct Listener {
    socket: HostListener,
    prepared: Mutex<Option<TcpStream>>,
    waiting: Arc<AtomicBool>,
}

/// One pending consumer per affine endpoint. Its reservation is released on
/// cancellation, a losing select arm, a failed preparation or completed commit.
struct ConsumerPermit(Arc<AtomicBool>);

impl ConsumerPermit {
    fn acquire(waiting: &Arc<AtomicBool>) -> Result<Self, NetError> {
        waiting
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| NetError::ResourceLimit)?;
        Ok(Self(waiting.clone()))
    }
}

impl Drop for ConsumerPermit {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub(crate) struct PreparedAccept {
    listener: Arc<Listener>,
    _permit: ConsumerPermit,
}

impl PreparedAccept {
    pub(crate) fn commit(self) -> Result<TcpStream, NetError> {
        self.listener
            .prepared
            .lock()
            .map_err(|_| NetError::Host)?
            .take()
            .ok_or(NetError::Host)
    }
}

impl Listener {
    pub(crate) fn listen(address: SocketAddress, backlog: i128) -> Result<Self, NetError> {
        let backlog = u32::try_from(backlog)
            .ok()
            .filter(|backlog| *backlog != 0)
            .ok_or(NetError::InvalidLimit)?;
        let socket = if address.ip().as_ip().is_ipv4() {
            TcpSocket::new_v4()
        } else {
            TcpSocket::new_v6()
        }
        .map_err(|error| io_error(&error))?;
        socket
            .bind(host_address(address))
            .map_err(|error| io_error(&error))?;
        let socket = socket.listen(backlog).map_err(|error| io_error(&error))?;
        Ok(Self {
            socket,
            prepared: Mutex::new(None),
            waiting: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddress, NetError> {
        tondo_address(self.socket.local_addr().map_err(|error| io_error(&error))?)
    }

    pub(crate) async fn prepare_accept(self: &Arc<Self>) -> Result<PreparedAccept, NetError> {
        let permit = ConsumerPermit::acquire(&self.waiting)?;
        let already_prepared = self.prepared.lock().map_err(|_| NetError::Host)?.is_some();
        if !already_prepared {
            let (stream, _) = self
                .socket
                .accept()
                .await
                .map_err(|error| io_error(&error))?;
            let stream = TcpStream::from_host(stream)?;
            *self.prepared.lock().map_err(|_| NetError::Host)? = Some(stream);
        }
        Ok(PreparedAccept {
            listener: self.clone(),
            _permit: permit,
        })
    }
}

struct TcpTransport {
    socket: HostStream,
    // The safe standard handle supplies read/both shutdown, which Tokio does
    // not expose. Both descriptors name one transport and die with its owner.
    shutdown: std::net::TcpStream,
}

pub(crate) struct TcpStream(Arc<TcpTransport>);
pub(crate) struct TcpReadHalf {
    transport: Arc<TcpTransport>,
    waiting: Arc<AtomicBool>,
}
pub(crate) struct TcpWriteHalf(Arc<TcpTransport>);

impl TcpStream {
    fn from_host(socket: HostStream) -> Result<Self, NetError> {
        let standard = socket.into_std().map_err(|error| io_error(&error))?;
        let shutdown = standard.try_clone().map_err(|error| io_error(&error))?;
        let socket = HostStream::from_std(standard).map_err(|error| io_error(&error))?;
        Ok(Self(Arc::new(TcpTransport { socket, shutdown })))
    }

    pub(crate) async fn connect(address: SocketAddress) -> Result<Self, NetError> {
        address.require_destination()?;
        Self::from_host(
            HostStream::connect(host_address(address))
                .await
                .map_err(|error| io_error(&error))?,
        )
    }

    pub(crate) fn split(self) -> (TcpReadHalf, TcpWriteHalf) {
        (
            TcpReadHalf {
                transport: self.0.clone(),
                waiting: Arc::new(AtomicBool::new(false)),
            },
            TcpWriteHalf(self.0),
        )
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddress, NetError> {
        tondo_address(
            self.0
                .socket
                .local_addr()
                .map_err(|error| io_error(&error))?,
        )
    }

    pub(crate) fn peer_address(&self) -> Result<SocketAddress, NetError> {
        tondo_address(
            self.0
                .socket
                .peer_addr()
                .map_err(|error| io_error(&error))?,
        )
    }

    pub(crate) fn shutdown(&self, how: Shutdown) -> Result<(), NetError> {
        self.0
            .shutdown
            .shutdown(how)
            .map_err(|error| io_error(&error))
    }

    pub(super) fn socket(&self) -> &HostStream {
        &self.0.socket
    }
}

fn read_buffer(size: usize) -> Result<Vec<u8>, NetError> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| NetError::ResourceLimit)?;
    bytes.resize(size, 0);
    Ok(bytes)
}

impl TcpReadHalf {
    /// Peek readiness only. Preparation neither consumes bytes nor advances EOF.
    pub(crate) async fn prepare_read(
        &self,
        max: i128,
        limits: NetLimits,
    ) -> Result<PreparedRead, NetError> {
        let size = limits.read_request(max)?;
        let permit = ConsumerPermit::acquire(&self.waiting)?;
        let bytes = read_buffer(size)?;
        let mut byte = [0];
        self.transport
            .socket
            .peek(&mut byte)
            .await
            .map_err(|error| io_error(&error))?;
        Ok(PreparedRead {
            transport: self.transport.clone(),
            bytes,
            _permit: permit,
        })
    }

    pub(crate) async fn read(&self, max: i128, limits: NetLimits) -> Result<ReadResult, NetError> {
        loop {
            if let Some(result) = self.prepare_read(max, limits).await?.commit()? {
                return Ok(result);
            }
        }
    }
}

pub(crate) struct PreparedRead {
    transport: Arc<TcpTransport>,
    bytes: Vec<u8>,
    _permit: ConsumerPermit,
}

impl PreparedRead {
    pub(crate) fn commit(mut self) -> Result<Option<ReadResult>, NetError> {
        let size = self.bytes.len();
        match self.transport.socket.try_read(&mut self.bytes) {
            Ok(length) => {
                self.bytes.truncate(length);
                tcp_read_result(self.bytes, size, length == 0).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(io_error(&error)),
        }
    }
}

impl TcpWriteHalf {
    pub(crate) async fn write(&self, bytes: &[u8]) -> Result<usize, NetError> {
        if bytes.is_empty() {
            return Ok(0);
        }
        loop {
            self.0
                .socket
                .writable()
                .await
                .map_err(|error| io_error(&error))?;
            match self.0.socket.try_write(bytes) {
                Ok(length) => return tcp_write_result(bytes.len(), length),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                Err(error) => return Err(io_error(&error)),
            }
        }
    }

    pub(crate) fn shutdown(&self) -> Result<(), NetError> {
        self.0
            .shutdown
            .shutdown(Shutdown::Write)
            .map_err(|error| io_error(&error))
    }
}

pub(crate) struct UdpSocket {
    socket: Arc<HostDatagramSocket>,
    waiting: Arc<AtomicBool>,
}

impl UdpSocket {
    pub(crate) fn bind(address: SocketAddress) -> Result<Self, NetError> {
        let standard =
            std::net::UdpSocket::bind(host_address(address)).map_err(|error| io_error(&error))?;
        standard
            .set_nonblocking(true)
            .map_err(|error| io_error(&error))?;
        let socket = HostDatagramSocket::from_std(standard).map_err(|error| io_error(&error))?;
        Ok(Self {
            socket: Arc::new(socket),
            waiting: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn local_address(&self) -> Result<SocketAddress, NetError> {
        tondo_address(self.socket.local_addr().map_err(|error| io_error(&error))?)
    }

    pub(crate) async fn send(
        &self,
        bytes: &[u8],
        destination: SocketAddress,
        limits: NetLimits,
    ) -> Result<(), NetError> {
        destination.require_destination()?;
        limits.datagram_size(bytes.len())?;
        let length = self
            .socket
            .send_to(bytes, host_address(destination))
            .await
            .map_err(|error| io_error(&error))?;
        udp_send_result(bytes.len(), length)
    }

    /// One extra byte distinguishes a valid bounded datagram from an oversized
    /// one. Even when a larger message is truncated by this private peek, no
    /// prefix is published as a valid Datagram.
    pub(crate) async fn prepare_receive(
        &self,
        limits: NetLimits,
    ) -> Result<PreparedDatagram, NetError> {
        let permit = ConsumerPermit::acquire(&self.waiting)?;
        let mut bytes = read_buffer(limits.max_datagram() + 1)?;
        self.socket
            .peek_from(&mut bytes)
            .await
            .map_err(|error| io_error(&error))?;
        Ok(PreparedDatagram {
            socket: self.socket.clone(),
            bytes,
            limits,
            _permit: permit,
        })
    }

    pub(crate) async fn receive(&self, limits: NetLimits) -> Result<Datagram, NetError> {
        loop {
            if let Some(result) = self.prepare_receive(limits).await?.commit()? {
                return Ok(result);
            }
        }
    }
}

pub(crate) struct PreparedDatagram {
    socket: Arc<HostDatagramSocket>,
    bytes: Vec<u8>,
    limits: NetLimits,
    _permit: ConsumerPermit,
}

impl PreparedDatagram {
    pub(crate) fn commit(mut self) -> Result<Option<Datagram>, NetError> {
        match self.socket.try_recv_from(&mut self.bytes) {
            Ok((length, source)) => {
                if length > self.limits.max_datagram() {
                    return Err(NetError::DatagramTooLarge);
                }
                self.bytes.truncate(length);
                Datagram::receive(self.bytes, tondo_address(source)?, length, self.limits).map(Some)
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(io_error(&error)),
        }
    }
}
