//! What `std.net` does in the runtime (standard library 14.4.6): TCP
//! connections and listeners, with TLS, UDP sockets, Unix sockets of both
//! kinds and names. A connection
//! is two streams of `std.io` in `Mode::Ready`, so a task waiting on the
//! network holds neither a worker nor a thread. Sockets without TLS are the
//! workers' own (`poll`): a task reads and writes them itself, and what a
//! socket could not take at once goes when it can. TLS and names use the
//! HTTP runtime's tokio, whose tasks move the bytes of a TLS stream between
//! it and two buffers.

use super::http::{runtime, text};
use super::native::{self, NativeObject};
use super::poll::{Direction, Registered};
use super::{ArrayHeader, Str, str_new};
use crate::io::{Host, Mode, Ready, StreamObject, byte_array, retain, run, run_until, wait_for};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

/// How many bytes a direction of a connection holds before it waits.
const HIGH: usize = 256 * 1024;
/// A datagram's largest payload, over IPv6.
const DATAGRAM: usize = 65_535;

// The codes of `NetError`, with 0 for success and `END` for the end of the
// data, which `std.net` turns into its variants.
const END: i64 = 1;
const NOT_FOUND: i64 = 2;
const REFUSED: i64 = 3;
const TIMED_OUT: i64 = 4;
const RESET: i64 = 5;
const CLOSED: i64 = 6;
const IN_USE: i64 = 7;
const UNREACHABLE: i64 = 8;
const INVALID_ADDRESS: i64 = 9;
const TLS: i64 = 10;
const IO: i64 = 11;

/// Why an operation failed: a code and the text some variants carry.
#[derive(Clone, Debug, Default)]
pub(crate) struct Failure {
    code: i64,
    detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for Failure {}

fn failure(code: i64, detail: impl Into<String>) -> Failure {
    Failure { code, detail: detail.into() }
}

impl From<&std::io::Error> for Failure {
    fn from(error: &std::io::Error) -> Failure {
        if let Some(known) = error.get_ref().and_then(|inner| inner.downcast_ref::<Failure>()) {
            return known.clone();
        }
        if let Some(tls) = error.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
            return failure(TLS, tls.to_string());
        }
        let code = match error.kind() {
            ErrorKind::ConnectionRefused => REFUSED,
            ErrorKind::TimedOut => TIMED_OUT,
            ErrorKind::ConnectionReset | ErrorKind::ConnectionAborted => RESET,
            ErrorKind::BrokenPipe | ErrorKind::NotConnected | ErrorKind::Interrupted => CLOSED,
            ErrorKind::AddrInUse => IN_USE,
            ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => UNREACHABLE,
            ErrorKind::AddrNotAvailable => INVALID_ADDRESS,
            _ => IO,
        };
        failure(code, error.to_string())
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Failure {
        Failure::from(&error)
    }
}

/// A deadline `ms` milliseconds from now, or none when negative.
fn deadline(ms: i64) -> Option<Instant> {
    (ms >= 0).then(|| Instant::now() + Duration::from_millis(ms as u64))
}

/// A future that fails with `TimedOut` at `deadline`.
async fn within<T>(deadline: Option<Instant>, work: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    match deadline {
        Some(at) => {
            tokio::time::timeout_at(at.into(), work).await.unwrap_or_else(|_| Err(failure(TIMED_OUT, "timed out")))
        }
        None => work.await,
    }
}

/// What a task gets for waiting on `work`: cancelling the task ends it.
fn waited<T>(work: impl Future<Output = Result<T, Failure>>) -> Result<T, Failure> {
    wait_for(work).unwrap_or_else(|| Err(failure(CLOSED, "the task was cancelled")))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The addresses of `host`, a name or an IP address, in the system's order.
async fn lookup(host: &str, port: u16) -> Result<Vec<SocketAddr>, Failure> {
    let found = tokio::net::lookup_host((host, port)).await.map_err(|_| failure(NOT_FOUND, host))?;
    let mut addresses: Vec<SocketAddr> = Vec::new();
    for address in found {
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    match addresses.is_empty() {
        true => Err(failure(NOT_FOUND, host)),
        false => Ok(addresses),
    }
}

fn port(port: i64) -> Result<u16, Failure> {
    u16::try_from(port).map_err(|_| failure(INVALID_ADDRESS, format!("port {port} is not between 0 and 65535")))
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

/// One direction of a TLS connection: the bytes between the TLS stream and
/// its stream of `std.io`, which the tokio task moving them and the
/// stream's host share.
#[derive(Default)]
struct Pipe {
    flow: Mutex<Flow>,
    changed: Notify,
}

#[derive(Default)]
struct Flow {
    /// Received and not read yet, or written and not sent yet.
    bytes: Vec<u8>,
    /// Sending: how many bytes the socket is taking now.
    sending: usize,
    /// Receiving, the other side ended; sending, the stream is done.
    end: bool,
    /// The stream let go of it: the task stops.
    gone: bool,
    failure: Option<Failure>,
}

impl Pipe {
    fn flow(&self) -> MutexGuard<'_, Flow> {
        lock(&self.flow)
    }

    fn change(&self, change: impl FnOnce(&mut Flow)) {
        change(&mut self.flow());
        self.changed.notify_waiters();
    }

    /// Waits until `ready` holds.
    async fn until(&self, ready: impl Fn(&Flow) -> bool) {
        loop {
            let notified = self.changed.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if ready(&self.flow()) {
                return;
            }
            notified.await;
        }
    }
}

type Tls = tokio_rustls::TlsStream<tokio::net::TcpStream>;

/// Moves what a TLS stream receives into `pipe` while it has room.
async fn receive(mut from: tokio::io::ReadHalf<Tls>, pipe: Arc<Pipe>) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        pipe.until(|flow| flow.bytes.len() < HIGH || flow.gone).await;
        let read = tokio::select! {
            read = from.read(&mut buffer) => read,
            _ = pipe.until(|flow| flow.gone) => return,
        };
        let mut over = true;
        pipe.change(|flow| match read {
            Ok(0) => flow.end = true,
            Ok(count) => {
                flow.bytes.extend_from_slice(&buffer[..count]);
                over = false;
            }
            Err(error) => flow.failure = Some(error.into()),
        });
        if over {
            return;
        }
    }
}

/// Sends what the stream leaves in `pipe`, and ends the TLS stream after
/// the stream is done.
async fn send(mut to: tokio::io::WriteHalf<Tls>, pipe: Arc<Pipe>) {
    loop {
        pipe.until(|flow| !flow.bytes.is_empty() || flow.end).await;
        let chunk = {
            let mut flow = pipe.flow();
            if flow.bytes.is_empty() {
                break;
            }
            let chunk = std::mem::take(&mut flow.bytes);
            flow.sending = chunk.len();
            chunk
        };
        let sent = match to.write_all(&chunk).await {
            Ok(()) => to.flush().await,
            failed => failed,
        };
        let failed = sent.is_err();
        pipe.change(|flow| {
            flow.sending = 0;
            if let Err(error) = sent {
                flow.failure = Some(error.into());
            }
        });
        if failed {
            return;
        }
    }
    let _ = to.shutdown().await;
}

/// A connected socket of the workers' poll: TCP or Unix.
trait Wire: mio::event::Source + Send + Sync + 'static {
    fn take(&self, buffer: &mut [u8]) -> std::io::Result<usize>;
    fn give(&self, data: &[u8]) -> std::io::Result<usize>;
    fn end(&self) -> std::io::Result<()>;
}

impl Wire for mio::net::TcpStream {
    fn take(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (&*self).read(buffer)
    }
    fn give(&self, data: &[u8]) -> std::io::Result<usize> {
        (&*self).write(data)
    }
    fn end(&self) -> std::io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

impl Wire for mio::net::UnixStream {
    fn take(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (&*self).read(buffer)
    }
    fn give(&self, data: &[u8]) -> std::io::Result<usize> {
        (&*self).write(data)
    }
    fn end(&self) -> std::io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// A socket of the workers' poll, and what its writing direction holds.
struct Tcp<S: Wire> {
    socket: Registered<S>,
    unsent: Mutex<Unsent<S>>,
}

struct Unsent<S: Wire> {
    /// Written and not sent yet.
    bytes: Vec<u8>,
    failure: Option<Failure>,
    /// The writing stream let go of the socket with bytes to send: it ends
    /// its direction after them, and holds it until then.
    keep: Option<Arc<Tcp<S>>>,
}

impl<S: Wire> Default for Unsent<S> {
    fn default() -> Self {
        Unsent { bytes: Vec::new(), failure: None, keep: None }
    }
}

impl<S: Wire> Tcp<S> {
    /// How much of `data` the socket takes now.
    fn send(&self, data: &[u8]) -> std::io::Result<usize> {
        let mut sent = 0;
        while sent < data.len() {
            match self.socket.attempt(Direction::Write, |socket| socket.give(&data[sent..])) {
                Ok(count) => {
                    sent += count;
                    super::diagnostics::NET_WRITTEN.add(count as i64);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            }
        }
        Ok(sent)
    }

    /// Sends what waits while the socket takes it.
    fn push(&self, unsent: &mut Unsent<S>) -> std::io::Result<()> {
        failed(&unsent.failure)?;
        match self.send(&unsent.bytes) {
            Ok(sent) => {
                unsent.bytes.drain(..sent);
                Ok(())
            }
            Err(error) => Err(fail(unsent, error)),
        }
    }

    /// When the socket can take more: sends what waits, and ends the
    /// direction of a stream that let go of it once nothing does.
    fn writable(&self) {
        let mut unsent = lock(&self.unsent);
        let _ = self.push(&mut unsent);
        if unsent.keep.is_some() && (unsent.bytes.is_empty() || unsent.failure.is_some()) {
            let _ = self.socket.io().end();
            let keep = unsent.keep.take();
            drop(unsent);
            drop(keep);
        }
    }
}

/// Notes that sending failed, which every write after says.
fn fail<S: Wire>(unsent: &mut Unsent<S>, error: std::io::Error) -> std::io::Error {
    let failure = Failure::from(error);
    unsent.bytes.clear();
    unsent.failure = Some(failure.clone());
    std::io::Error::other(failure)
}

/// The reading half of a TCP connection as the host of a stream, which
/// reads the socket itself.
struct TcpIn<S: Wire> {
    tcp: Arc<Tcp<S>>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The writing half of a TCP connection as the host of a stream, which
/// writes the socket itself and leaves what it could not take to send when
/// it can.
struct TcpOut<S: Wire> {
    tcp: Arc<Tcp<S>>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The reading half of a TLS connection as the host of a stream.
struct Inbox {
    pipe: Arc<Pipe>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The writing half of a TLS connection as the host of a stream.
struct Outbox {
    pipe: Arc<Pipe>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The error of a host that would block: what it gave up with, or
/// `WouldBlock` after noting that it waits.
fn would_block(waiting: &mut bool, gave_up: Option<ErrorKind>) -> std::io::Error {
    gave_up
        .unwrap_or_else(|| {
            *waiting = true;
            ErrorKind::WouldBlock
        })
        .into()
}

fn failed(failure: &Option<Failure>) -> std::io::Result<()> {
    failure.as_ref().map_or(Ok(()), |failure| Err(std::io::Error::other(failure.clone())))
}

impl<S: Wire> Host for TcpIn<S> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self.tcp.socket.attempt(Direction::Read, |socket| socket.take(buffer)) {
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(would_block(&mut self.waiting, self.gave_up)),
            read => {
                super::diagnostics::NET_READ.add(*read.as_ref().unwrap_or(&0) as i64);
                read
            }
        }
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.waiting).then(|| Box::pin(self.tcp.socket.ready(Direction::Read)) as Ready)
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl Host for Inbox {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut flow = self.pipe.flow();
        if !flow.bytes.is_empty() {
            let count = buffer.len().min(flow.bytes.len());
            buffer[..count].copy_from_slice(&flow.bytes[..count]);
            flow.bytes.drain(..count);
            drop(flow);
            self.pipe.changed.notify_waiters();
            super::diagnostics::NET_READ.add(count as i64);
            return Ok(count);
        }
        failed(&flow.failure)?;
        if flow.end {
            return Ok(0);
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    fn blocked(&mut self) -> Option<Ready> {
        let pipe = self.pipe.clone();
        std::mem::take(&mut self.waiting).then(|| {
            Box::pin(
                async move { pipe.until(|flow| !flow.bytes.is_empty() || flow.end || flow.failure.is_some()).await },
            ) as Ready
        })
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.pipe.change(|flow| flow.gone = true);
    }
}

impl<S: Wire> Host for TcpOut<S> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut unsent = lock(&self.tcp.unsent);
        self.tcp.push(&mut unsent)?;
        let mut rest = data;
        if unsent.bytes.is_empty() {
            match self.tcp.send(data) {
                Ok(sent) => rest = &data[sent..],
                Err(error) => return Err(fail(&mut unsent, error)),
            }
        }
        if rest.is_empty() {
            return Ok(());
        }
        if unsent.bytes.is_empty() || unsent.bytes.len() + rest.len() <= HIGH {
            unsent.bytes.extend_from_slice(rest);
            return Ok(());
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    /// Waits until the socket took everything.
    fn flush(&mut self) -> std::io::Result<()> {
        let mut unsent = lock(&self.tcp.unsent);
        self.tcp.push(&mut unsent)?;
        if unsent.bytes.is_empty() {
            return Ok(());
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    fn blocked(&mut self) -> Option<Ready> {
        std::mem::take(&mut self.waiting).then(|| Box::pin(self.tcp.socket.ready(Direction::Write)) as Ready)
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl<S: Wire> Drop for TcpOut<S> {
    fn drop(&mut self) {
        let mut unsent = lock(&self.tcp.unsent);
        if unsent.bytes.is_empty() || unsent.failure.is_some() {
            let _ = self.tcp.socket.io().end();
        } else {
            unsent.keep = Some(self.tcp.clone());
        }
    }
}

impl Host for Outbox {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut flow = self.pipe.flow();
        failed(&flow.failure)?;
        let queued = flow.bytes.len() + flow.sending;
        if queued == 0 || queued + data.len() <= HIGH {
            flow.bytes.extend_from_slice(data);
            drop(flow);
            super::diagnostics::NET_WRITTEN.add(data.len() as i64);
            self.pipe.changed.notify_waiters();
            return Ok(());
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    /// Waits until the TLS stream took everything.
    fn flush(&mut self) -> std::io::Result<()> {
        let flow = self.pipe.flow();
        failed(&flow.failure)?;
        if flow.bytes.is_empty() && flow.sending == 0 {
            return Ok(());
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    fn blocked(&mut self) -> Option<Ready> {
        let pipe = self.pipe.clone();
        std::mem::take(&mut self.waiting).then(|| {
            Box::pin(async move {
                pipe.until(|flow| (flow.bytes.is_empty() && flow.sending == 0) || flow.failure.is_some()).await
            }) as Ready
        })
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl Drop for Outbox {
    fn drop(&mut self) {
        self.pipe.change(|flow| flow.end = true);
    }
}

/// A stream of `std.io` this module holds a reference to.
struct Held(*mut StreamObject);

unsafe impl Send for Held {}
unsafe impl Sync for Held {}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { crate::io::tondo_rt_stream_release(self.0) }
    }
}

/// Where a socket is: an IP address and port, or the path of a Unix socket,
/// empty for one without a name.
#[derive(Clone)]
enum Endpoint {
    Ip(SocketAddr),
    Path(String),
}

impl From<SocketAddr> for Endpoint {
    fn from(address: SocketAddr) -> Endpoint {
        Endpoint::Ip(address)
    }
}

impl From<std::os::unix::net::SocketAddr> for Endpoint {
    fn from(address: std::os::unix::net::SocketAddr) -> Endpoint {
        Endpoint::Path(address.as_pathname().map(|p| p.display().to_string()).unwrap_or_default())
    }
}

/// A connection: its streams, its addresses, and the last failure of each
/// direction.
struct Connection {
    reader: Held,
    writer: Held,
    local: Endpoint,
    remote: Endpoint,
    failures: [Mutex<Failure>; 2],
    /// The socket of a plain TCP connection, which `startTls` takes over.
    socket: Option<std::os::fd::RawFd>,
    /// Whether `startTls` took it over, after which it is closed.
    upgraded: std::sync::atomic::AtomicBool,
}

impl Connection {
    fn new(reader: Box<dyn Host>, writer: Box<dyn Host>, local: Endpoint, remote: Endpoint) -> Connection {
        super::diagnostics::NET_CONNECTIONS.add(1);
        Connection {
            reader: Held(crate::io::stream(reader, Mode::Ready, false)),
            writer: Held(crate::io::stream(writer, Mode::Ready, false)),
            local,
            remote,
            failures: Default::default(),
            socket: None,
            upgraded: Default::default(),
        }
    }
}

/// A connection over a TCP socket.
fn tcp(socket: Registered<mio::net::TcpStream>) -> Result<Connection, Failure> {
    use std::os::fd::AsRawFd;
    let _ = socket.io().set_nodelay(true);
    let (local, remote) = (socket.io().local_addr()?, socket.io().peer_addr()?);
    let fd = socket.io().as_raw_fd();
    let mut connection = link(socket, local.into(), remote.into());
    connection.socket = Some(fd);
    Ok(connection)
}

/// A connection over a Unix socket.
fn unix(socket: Registered<mio::net::UnixStream>) -> Result<Connection, Failure> {
    let (local, remote) = (socket.io().local_addr()?, socket.io().peer_addr()?);
    Ok(link(socket, local.into(), remote.into()))
}

/// A connection over a socket the task reads and writes itself.
fn link<S: Wire>(socket: Registered<S>, local: Endpoint, remote: Endpoint) -> Connection {
    let tcp = Arc::new(Tcp { socket, unsent: Mutex::new(Unsent::default()) });
    let weak = Arc::downgrade(&tcp);
    tcp.socket.on_writable(Arc::new(move || {
        if let Some(tcp) = weak.upgrade() {
            tcp.writable();
        }
    }));
    let reader = TcpIn { tcp: tcp.clone(), waiting: false, gave_up: None };
    let writer = TcpOut { tcp, waiting: false, gave_up: None };
    Connection::new(Box::new(reader), Box::new(writer), local, remote)
}

/// A connection over TLS, and the pipes `secure` moves its bytes through
/// once the handshake is over.
fn tls(stream: &tokio::net::TcpStream) -> Result<(Connection, Arc<Pipe>, Arc<Pipe>), Failure> {
    let _ = stream.set_nodelay(true);
    let (local, remote) = (stream.local_addr()?, stream.peer_addr()?);
    let (inbox, outbox) = (Arc::new(Pipe::default()), Arc::new(Pipe::default()));
    let reader = Inbox { pipe: inbox.clone(), waiting: false, gave_up: None };
    let writer = Outbox { pipe: outbox.clone(), waiting: false, gave_up: None };
    Ok((Connection::new(Box::new(reader), Box::new(writer), local.into(), remote.into()), inbox, outbox))
}

/// Starts the tasks that move the bytes of a TLS stream.
fn secure(stream: Tls, inbox: Arc<Pipe>, outbox: Arc<Pipe>) {
    let (from, to) = tokio::io::split(stream);
    runtime().spawn(receive(from, inbox));
    runtime().spawn(send(to, outbox));
}

/// Connects to each address of `host` in turn, until one answers.
async fn open<S, F: Future<Output = std::io::Result<S>>>(
    host: &str,
    port: u16,
    connect: impl Fn(SocketAddr) -> F,
) -> Result<S, Failure> {
    let mut last = failure(NOT_FOUND, host);
    for address in lookup(host, port).await? {
        match connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error.into(),
        }
    }
    Err(last)
}

/// A TCP connection to `address` on the workers' poll.
async fn connect(address: SocketAddr) -> std::io::Result<Registered<mio::net::TcpStream>> {
    let socket = Registered::new(mio::net::TcpStream::connect(address)?)?;
    loop {
        socket.ready(Direction::Write).await;
        if let Some(error) = socket.io().take_error()? {
            return Err(error);
        }
        // Connected once it has a peer.
        let connected = socket.attempt(Direction::Write, |socket| match socket.peer_addr() {
            Err(error) if error.kind() == ErrorKind::NotConnected => Err(ErrorKind::WouldBlock.into()),
            connected => connected,
        });
        match connected {
            Ok(_) => return Ok(socket),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
}

/// A socket of the workers' poll as one of tokio's, for TLS.
fn tokio_stream(stream: mio::net::TcpStream) -> std::io::Result<tokio::net::TcpStream> {
    let _context = runtime().enter();
    tokio::net::TcpStream::from_std(std::net::TcpStream::from(std::os::fd::OwnedFd::from(stream)))
}

/// What verifies a TLS connection to `host`: the roots, the authorities
/// of `trust`, and the certificate and key it shows, as `http::tls` takes
/// them.
fn verifier(
    host: &str,
    trust: &str,
    cert: &str,
    key: &str,
    minimum: i64,
) -> Result<(tokio_rustls::TlsConnector, rustls_pki_types::ServerName<'static>), Failure> {
    let mut config = super::http::tls(trust, cert, key, minimum).map_err(|problem| failure(TLS, problem))?;
    config.alpn_protocols.clear();
    let name = rustls_pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| failure(INVALID_ADDRESS, format!("`{host}` is not a name TLS can verify")))?;
    Ok((tokio_rustls::TlsConnector::from(Arc::new(config)), name))
}

/// The handshake of a client over `stream`, and its connection.
async fn handshake(
    stream: tokio::net::TcpStream,
    connector: tokio_rustls::TlsConnector,
    name: rustls_pki_types::ServerName<'static>,
) -> Result<Connection, Failure> {
    let (connection, inbox, outbox) = tls(&stream)?;
    let stream = connector.connect(name, stream).await.map_err(|error| {
        let mut why = Failure::from(&error).detail;
        if why.contains("UnknownIssuer") {
            why.push_str("; if an authority of your own signed it, pass its PEM as `trust:`");
        }
        failure(TLS, why)
    })?;
    secure(stream.into(), inbox, outbox);
    Ok(connection)
}

/// `connect` and `connectTls` (`tls` 1): a connection, or why there is
/// none; `trust`, `cert`, `key` and `minimum` are its TLS's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_connect(
    host: *const Str,
    port_number: i64,
    timeout_ms: i64,
    tls_on: u8,
    trust: *const Str,
    cert: *const Str,
    key: *const Str,
    minimum: i64,
) -> *mut NativeObject {
    let host = text(host);
    let deadline = deadline(timeout_ms);
    let connected = port(port_number).and_then(|port| {
        let verify = match tls_on {
            0 => None,
            _ => Some(verifier(&host, &text(trust), &text(cert), &text(key), minimum)?),
        };
        waited(within(deadline, async move {
            let Some((connector, name)) = verify else { return tcp(open(&host, port, connect).await?) };
            handshake(open(&host, port, tokio::net::TcpStream::connect).await?, connector, name).await
        }))
    });
    native::new(Net::from(connected.map(Net::Connection)))
}

/// `startTls`: the connection over TLS from here on, as a client of `host`;
/// the plain one is closed. Bytes it held unread or unsent are lost, so it
/// starts when the protocol says, with nothing in flight.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_start_tls(
    object: *mut NativeObject,
    host: *const Str,
    trust: *const Str,
    cert: *const Str,
    key: *const Str,
    minimum: i64,
    timeout_ms: i64,
) -> *mut NativeObject {
    use std::os::fd::FromRawFd;
    let Some(connection) = connection_of(object) else {
        return native::new(Net::Failed(failure(CLOSED, "closed")));
    };
    let Some(fd) = connection.socket else {
        return native::new(Net::Failed(failure(TLS, "only a plain TCP connection starts TLS")));
    };
    let host = text(host);
    let upgraded = verifier(&host, &text(trust), &text(cert), &text(key), minimum).and_then(|(connector, name)| {
        // The socket goes on under a descriptor of its own; the plain
        // connection closes its own without ending the socket.
        let duplicate = unsafe { libc::dup(fd) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        connection.upgraded.store(true, std::sync::atomic::Ordering::Release);
        let socket = unsafe { std::net::TcpStream::from_raw_fd(duplicate) };
        socket.set_nonblocking(true)?;
        let stream = {
            let _context = runtime().enter();
            tokio::net::TcpStream::from_std(socket)?
        };
        waited(within(deadline(timeout_ms), handshake(stream, connector, name)))
    });
    native::new(Net::from(upgraded.map(Net::Connection)))
}

/// `connectUnix`: a connection to the Unix socket at `path`, or why there
/// is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_connect_unix(path: *const Str, timeout_ms: i64) -> *mut NativeObject {
    let path = text(path);
    let connected = waited(within(deadline(timeout_ms), async move {
        let socket = match mio::net::UnixStream::connect(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let why = format!("there is no Unix socket at `{path}`: does the program that listens on it run?");
                return Err(failure(IO, why));
            }
            socket => Registered::new(socket?)?,
        };
        loop {
            socket.ready(Direction::Write).await;
            if let Some(error) = socket.io().take_error()? {
                return Err(error.into());
            }
            // Connected once it has a peer.
            let connected = socket.attempt(Direction::Write, |socket| match socket.peer_addr() {
                Err(error) if error.kind() == ErrorKind::NotConnected => Err(ErrorKind::WouldBlock.into()),
                connected => connected,
            });
            match connected {
                Ok(_) => return unix(socket),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
    }));
    native::new(Net::from(connected.map(Net::Connection)))
}

/// Makes room for a Unix socket at `path`: a socket nobody answers on any
/// more, which a program that ended left, goes; another file is in use.
pub(crate) fn make_room(path: &str, datagram: bool) -> Result<(), Failure> {
    use std::os::unix::fs::FileTypeExt;
    let Ok(metadata) = std::fs::symlink_metadata(path) else { return Ok(()) };
    if !metadata.file_type().is_socket() {
        return Err(failure(IN_USE, format!("`{path}` is a file that is not a socket")));
    }
    let answers = match datagram {
        true => std::os::unix::net::UnixDatagram::unbound().and_then(|socket| socket.connect(path)).is_ok(),
        false => std::os::unix::net::UnixStream::connect(path).is_ok(),
    };
    if answers {
        return Err(failure(IN_USE, path));
    }
    std::fs::remove_file(path)?;
    Ok(())
}

/// Removes the file of a Unix socket with a name.
fn unlink(endpoint: &Endpoint) {
    if let Endpoint::Path(path) = endpoint
        && !path.is_empty()
    {
        let _ = std::fs::remove_file(path);
    }
}

/// The direction of a connection's operations: reading or writing.
fn direction(connection: &Connection, writing: bool) -> &Mutex<Failure> {
    &connection.failures[writing as usize]
}

/// Notes how an operation of a direction ended: 0, `END` or a failure.
fn note(connection: &Connection, writing: bool, outcome: Result<bool, Failure>) -> i64 {
    let noted = match outcome {
        Ok(more) => failure(if more { 0 } else { END }, ""),
        Err(failure) => failure,
    };
    let code = noted.code;
    *lock(direction(connection, writing)) = noted;
    code
}

fn connection_of<'a>(object: *mut NativeObject) -> Option<&'a Connection> {
    match native::get::<Net>(object) {
        Some(Net::Connection(connection)) if !connection.upgraded.load(std::sync::atomic::Ordering::Acquire) => {
            Some(connection)
        }
        _ => None,
    }
}

/// Runs an operation on a stream of a connection, until `deadline`.
fn on<R: Send + 'static>(
    stream: &Held,
    deadline: Option<Instant>,
    op: impl FnMut(&mut crate::io::StreamState) -> std::io::Result<R> + Send + 'static,
) -> Result<R, Failure> {
    run_until(stream.0, deadline, op).map_err(|error| match error.kind() {
        ErrorKind::NotConnected => failure(CLOSED, "closed"),
        _ => Failure::from(error),
    })
}

/// `read`: up to `max` bytes, or none at the end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_read(object: *mut NativeObject, max: i64, timeout_ms: i64) -> *mut ArrayHeader {
    let Some(connection) = connection_of(object) else { return byte_array(&[]) };
    let max = max.max(1) as usize;
    let read = on(&connection.reader, deadline(timeout_ms), move |s| s.read(max));
    let bytes = read.as_ref().ok().cloned().flatten().unwrap_or_default();
    note(connection, false, read.map(|bytes| bytes.is_some()));
    byte_array(&bytes)
}

/// `readLine`: the next line without its `\n` or `\r\n`, or none at the end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_read_line(object: *mut NativeObject, timeout_ms: i64) -> *mut Str {
    let Some(connection) = connection_of(object) else { return str_new(b"") };
    let read = on(&connection.reader, deadline(timeout_ms), |s| s.read_line());
    let line = read.and_then(|line| match line {
        Some(line) => String::from_utf8(line).map(Some).map_err(|_| failure(IO, "a line that is not UTF-8")),
        None => Ok(None),
    });
    let text = line.as_ref().ok().cloned().flatten().unwrap_or_default();
    note(connection, false, line.map(|line| line.is_some()));
    str_new(text.as_bytes())
}

/// `readAll`: everything until the other side ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_read_all(object: *mut NativeObject, timeout_ms: i64) -> *mut ArrayHeader {
    let Some(connection) = connection_of(object) else { return byte_array(&[]) };
    let read = on(&connection.reader, deadline(timeout_ms), |s| s.read_rest());
    let bytes = read.as_ref().ok().cloned().unwrap_or_default();
    note(connection, false, read.map(|_| true));
    byte_array(&bytes)
}

fn write(object: *mut NativeObject, data: Vec<u8>, timeout_ms: i64) -> i64 {
    let Some(connection) = connection_of(object) else { return CLOSED };
    let written = on(&connection.writer, deadline(timeout_ms), move |s| s.write(&data));
    note(connection, true, written.map(|_| true))
}

/// `write`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_write(object: *mut NativeObject, text: *const Str, timeout_ms: i64) -> i64 {
    write(object, unsafe { super::str_bytes(text) }.to_vec(), timeout_ms)
}

/// `writeBytes`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_write_bytes(
    object: *mut NativeObject,
    data: *mut ArrayHeader,
    timeout_ms: i64,
) -> i64 {
    write(object, unsafe { crate::io::array_bytes(data) }, timeout_ms)
}

/// `close` (`both` 1) and `closeWrite`: sends what is pending, then ends
/// the connection or its writing direction.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_close(object: *mut NativeObject, both: u8, timeout_ms: i64) -> i64 {
    let Some(connection) = connection_of(object) else { return 0 };
    let closed = on(&connection.writer, deadline(timeout_ms), |s| s.close());
    if both != 0 {
        run(connection.reader.0, |s| {
            let _ = s.close();
        });
    }
    note(connection, true, closed.map(|_| true))
}

/// `reader()`: the stream the connection's reads go through.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_reader(object: *mut NativeObject) -> *mut StreamObject {
    connection_of(object).map_or_else(|| crate::io::tondo_rt_io_discard(), |c| retain(c.reader.0))
}

/// `writer()`: the stream the connection's writes go through.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_writer(object: *mut NativeObject) -> *mut StreamObject {
    connection_of(object).map_or_else(|| crate::io::tondo_rt_io_discard(), |c| retain(c.writer.0))
}

// ---------------------------------------------------------------------------
// Listeners
// ---------------------------------------------------------------------------

/// A listener, until it is closed, and a TLS acceptor when it has one.
struct Listener {
    listener: Mutex<Option<Acceptor>>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    local: Endpoint,
    closed: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        unlink(&self.local);
    }
}

/// The socket a listener accepts on: TCP or Unix.
#[derive(Clone)]
enum Acceptor {
    Tcp(Arc<Registered<mio::net::TcpListener>>),
    Unix(Arc<Registered<mio::net::UnixListener>>),
}

/// A connection accepted, before it is a `Connection`.
enum Accepted {
    Tcp(mio::net::TcpStream),
    Unix(mio::net::UnixStream),
}

impl Acceptor {
    fn accept(&self) -> std::io::Result<Accepted> {
        match self {
            Acceptor::Tcp(socket) => socket.attempt(Direction::Read, |l| l.accept()).map(|(s, _)| Accepted::Tcp(s)),
            Acceptor::Unix(socket) => socket.attempt(Direction::Read, |l| l.accept()).map(|(s, _)| Accepted::Unix(s)),
        }
    }

    async fn ready(&self) {
        match self {
            Acceptor::Tcp(socket) => socket.ready(Direction::Read).await,
            Acceptor::Unix(socket) => socket.ready(Direction::Read).await,
        }
    }
}

/// A listener whose queue of connections not accepted yet is as long as
/// the system allows, so a burst of them waits instead of being dropped.
fn bind(address: SocketAddr) -> std::io::Result<Registered<mio::net::TcpListener>> {
    let socket = socket2::Socket::new(socket2::Domain::for_address(address), socket2::Type::STREAM, None)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&address.into())?;
    socket.listen(65_535)?;
    Registered::new(mio::net::TcpListener::from_std(socket.into()))
}

/// `listen`: a listener on `host:port`, `0` for any free port, over TLS
/// when it has a certificate and a key, as `server::acceptor` takes them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_listen(
    host: *const Str,
    port_number: i64,
    cert: *const Str,
    key: *const Str,
    clients: *const Str,
    minimum: i64,
) -> *mut NativeObject {
    let (host, cert, key, clients) = (text(host), text(cert), text(key), text(clients));
    let listening = port(port_number).and_then(|port| {
        let tls = match cert.is_empty() {
            true => None,
            false => Some(
                super::server::acceptor(&cert, &key, &clients, minimum, false)
                    .map_err(|problem| failure(TLS, problem))?,
            ),
        };
        let listener = waited(async move {
            let mut last = failure(NOT_FOUND, host.as_str());
            for address in lookup(&host, port).await? {
                match bind(address) {
                    Ok(listener) => return Ok(listener),
                    Err(error) => last = error.into(),
                }
            }
            Err(last)
        })?;
        let local = listener.io().local_addr()?.into();
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        let listener = Mutex::new(Some(Acceptor::Tcp(Arc::new(listener))));
        Ok(Net::Listener(Listener { listener, tls, local, closed }))
    });
    native::new(Net::from(listening))
}

/// `listenUnix`: a listener on a Unix socket at `path`, or why there is
/// none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_listen_unix(path: *const Str) -> *mut NativeObject {
    let path = text(path);
    let listening = make_room(&path, false).and_then(|()| {
        let listener = Registered::new(mio::net::UnixListener::bind(&path)?)?;
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        let listener = Mutex::new(Some(Acceptor::Unix(Arc::new(listener))));
        Ok(Net::Listener(Listener { listener, tls: None, local: Endpoint::Path(path), closed }))
    });
    native::new(Net::from(listening))
}

/// `accept`, and the next connection when iterating (`keep` 1), which
/// waits past failures to accept: a connection, or why there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_accept(
    object: *mut NativeObject,
    timeout_ms: i64,
    keep: u8,
) -> *mut NativeObject {
    let Some(Net::Listener(listener)) = native::get::<Net>(object) else {
        return native::new(Net::Failed(failure(CLOSED, "closed")));
    };
    let Some(socket) = lock(&listener.listener).clone() else {
        return native::new(Net::Failed(failure(CLOSED, "closed")));
    };
    let (acceptor, closed) = (listener.tls.clone(), listener.closed.clone());
    let accepted = waited(within(deadline(timeout_ms), async move {
        let mut closing = closed.subscribe();
        loop {
            if *closing.borrow() {
                return Err(failure(CLOSED, "closed"));
            }
            match socket.accept() {
                Ok(accepted) => return Ok(accepted),
                Err(error) if error.kind() == ErrorKind::WouldBlock => tokio::select! {
                    _ = socket.ready() => {}
                    _ = closing.changed() => {}
                },
                Err(error) if keep == 0 => return Err(error.into()),
                // Out of descriptors, or a connection reset before it was
                // accepted: the next one may work.
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }));
    let opened = accepted.and_then(|accepted| {
        let stream = match accepted {
            Accepted::Unix(stream) => return unix(Registered::new(stream)?).map(Net::Connection),
            Accepted::Tcp(stream) => stream,
        };
        let Some(acceptor) = acceptor else { return tcp(Registered::new(stream)?).map(Net::Connection) };
        let stream = tokio_stream(stream)?;
        let (connection, inbox, outbox) = tls(&stream)?;
        // The handshake happens in its own task, so a slow client does not
        // hold up the next one; its failure is the first read's.
        runtime().spawn(async move {
            match acceptor.accept(stream).await {
                Ok(stream) => secure(stream.into(), inbox, outbox),
                Err(error) => {
                    let why = failure(TLS, Failure::from(&error).detail);
                    inbox.change(|flow| flow.failure = Some(why.clone()));
                    outbox.change(|flow| flow.failure = Some(why));
                }
            }
        });
        Ok(Net::Connection(connection))
    });
    native::new(Net::from(opened))
}

/// `close` of a listener or a socket: it lets go of its port once waiting
/// operations end, which they do with `Closed`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_stop(object: *mut NativeObject) {
    match native::get::<Net>(object) {
        Some(Net::Listener(listener)) => {
            lock(&listener.listener).take();
            listener.closed.send_replace(true);
            unlink(&listener.local);
        }
        Some(Net::Socket(socket)) => {
            lock(&socket.socket).take();
            socket.closed.send_replace(true);
            unlink(&socket.local);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// UDP
// ---------------------------------------------------------------------------

/// A socket of datagrams, until it is closed.
struct Socket {
    socket: Mutex<Option<Datagrams>>,
    local: Endpoint,
    closed: Arc<tokio::sync::watch::Sender<bool>>,
    failure: Mutex<Failure>,
}

impl Drop for Socket {
    fn drop(&mut self) {
        unlink(&self.local);
    }
}

/// A socket of datagrams: UDP or Unix.
#[derive(Clone)]
enum Datagrams {
    Udp(Arc<Registered<mio::net::UdpSocket>>),
    Unix(Arc<Registered<mio::net::UnixDatagram>>),
}

/// Lets a socket send datagrams as large as one can be: macOS sends none
/// larger than the send buffer, 9 216 bytes unless raised.
fn room(socket: socket2::SockRef) {
    if socket.send_buffer_size().is_ok_and(|size| size < 2 * DATAGRAM) {
        let _ = socket.set_send_buffer_size(2 * DATAGRAM);
    }
}

/// `bind`: a UDP socket on `host:port`, or why there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_bind(host: *const Str, port_number: i64) -> *mut NativeObject {
    let host = text(host);
    let bound = port(port_number).and_then(|port| {
        let socket = waited(async move {
            let mut last = failure(NOT_FOUND, host.as_str());
            for address in lookup(&host, port).await? {
                match mio::net::UdpSocket::bind(address) {
                    Ok(socket) => {
                        room(socket2::SockRef::from(&socket));
                        return Ok(Registered::new(socket)?);
                    }
                    Err(error) => last = error.into(),
                }
            }
            Err(last)
        })?;
        let local = socket.io().local_addr()?.into();
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        let socket = Mutex::new(Some(Datagrams::Udp(Arc::new(socket))));
        Ok(Net::Socket(Socket { socket, local, closed, failure: Default::default() }))
    });
    native::new(Net::from(bound))
}

/// `bindUnix`: a socket of datagrams at the Unix socket `path`, or without
/// `named` one without a name, which only sends; or why there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_bind_unix(path: *const Str, named: u8) -> *mut NativeObject {
    let path = if named != 0 { text(path) } else { String::new() };
    let bound = (|| {
        let socket = if path.is_empty() {
            mio::net::UnixDatagram::unbound()?
        } else {
            make_room(&path, true)?;
            mio::net::UnixDatagram::bind(&path)?
        };
        room(socket2::SockRef::from(&socket));
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        let socket = Mutex::new(Some(Datagrams::Unix(Arc::new(Registered::new(socket)?))));
        Ok(Net::Socket(Socket { socket, local: Endpoint::Path(path), closed, failure: Default::default() }))
    })();
    native::new(Net::from(bound))
}

/// `sendTo`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_send_to(
    object: *mut NativeObject,
    data: *mut ArrayHeader,
    ip: *const Str,
    port_number: i64,
) -> i64 {
    let Some(Net::Socket(socket)) = native::get::<Net>(object) else { return CLOSED };
    let data = unsafe { crate::io::array_bytes(data) };
    let (sending, ip) = (lock(&socket.socket).clone(), text(ip));
    let sent = port(port_number).and_then(|port| {
        let address = ip
            .parse::<std::net::IpAddr>()
            .map_err(|_| failure(INVALID_ADDRESS, format!("`{ip}` is not an IP address")))?;
        let to = SocketAddr::new(address, port);
        match sending.ok_or_else(|| failure(CLOSED, "closed"))? {
            Datagrams::Udp(sending) => send_datagram(sending, data, move |socket, data| socket.send_to(data, to)),
            Datagrams::Unix(_) => {
                Err(failure(INVALID_ADDRESS, "a Unix socket sends to a path: `net.unixAddress(path)`"))
            }
        }
    });
    noted(socket, sent)
}

/// `sendTo` the Unix socket at `path`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_send_to_path(
    object: *mut NativeObject,
    data: *mut ArrayHeader,
    path: *const Str,
) -> i64 {
    let Some(Net::Socket(socket)) = native::get::<Net>(object) else { return CLOSED };
    let (data, path) = (unsafe { crate::io::array_bytes(data) }, text(path));
    let sent = match lock(&socket.socket).clone() {
        None => Err(failure(CLOSED, "closed")),
        Some(Datagrams::Unix(sending)) => send_datagram(sending, data, move |socket, data| socket.send_to(data, &path)),
        Some(Datagrams::Udp(_)) => Err(failure(
            INVALID_ADDRESS,
            "a UDP socket sends to an IP address; a Unix socket comes from `net.bindUnix`",
        )),
    };
    noted(socket, sent)
}

/// Sends a datagram, whole, waiting while the socket cannot take it.
fn send_datagram<S: mio::event::Source + Send + Sync + 'static>(
    socket: Arc<Registered<S>>,
    data: Vec<u8>,
    send: impl Fn(&S, &[u8]) -> std::io::Result<usize> + Send + 'static,
) -> Result<(), Failure> {
    if data.len() > DATAGRAM {
        let why = format!("a datagram of {} bytes; one holds at most {DATAGRAM}", data.len());
        return Err(failure(IO, why));
    }
    waited(async move {
        loop {
            match socket.attempt(Direction::Write, |s| send(s, &data)) {
                Err(error) if error.kind() == ErrorKind::WouldBlock => socket.ready(Direction::Write).await,
                sent => {
                    super::diagnostics::NET_WRITTEN.add(*sent.as_ref().unwrap_or(&0) as i64);
                    return Ok(sent.map(|_| ())?);
                }
            }
        }
    })
}

/// Notes how a socket's last `sendTo` ended, and gives its code.
fn noted(socket: &Socket, sent: Result<(), Failure>) -> i64 {
    let noted = sent.err().unwrap_or_default();
    let code = noted.code;
    *lock(&socket.failure) = noted;
    code
}

impl Datagrams {
    fn receive(&self, buffer: &mut [u8]) -> std::io::Result<(usize, Endpoint)> {
        match self {
            Datagrams::Udp(s) => s.attempt(Direction::Read, |s| s.recv_from(buffer)).map(|(n, from)| (n, from.into())),
            Datagrams::Unix(s) => s.attempt(Direction::Read, |s| s.recv_from(buffer)).map(|(n, from)| (n, from.into())),
        }
    }

    async fn ready(&self) {
        match self {
            Datagrams::Udp(socket) => socket.ready(Direction::Read).await,
            Datagrams::Unix(socket) => socket.ready(Direction::Read).await,
        }
    }
}

/// A datagram received, and who sent it.
struct Datagram {
    bytes: Vec<u8>,
    from: Endpoint,
}

/// `receive`: the next datagram, whole, or why there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_receive(object: *mut NativeObject, timeout_ms: i64) -> *mut NativeObject {
    let Some(Net::Socket(socket)) = native::get::<Net>(object) else {
        return native::new(Net::Failed(failure(CLOSED, "closed")));
    };
    let Some(receiving) = lock(&socket.socket).clone() else {
        return native::new(Net::Failed(failure(CLOSED, "closed")));
    };
    let closed = socket.closed.clone();
    let received = waited(within(deadline(timeout_ms), async move {
        let mut closing = closed.subscribe();
        let mut buffer = vec![0; DATAGRAM];
        loop {
            if *closing.borrow() {
                return Err(failure(CLOSED, "closed"));
            }
            match receiving.receive(&mut buffer) {
                Ok((count, from)) => {
                    super::diagnostics::NET_READ.add(count as i64);
                    buffer.truncate(count);
                    return Ok(Net::Datagram(Datagram { bytes: buffer, from }));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => tokio::select! {
                    _ = receiving.ready() => {}
                    _ = closing.changed() => {}
                },
                Err(error) => return Err(error.into()),
            }
        }
    }));
    native::new(Net::from(received))
}

/// A datagram's bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_datagram(object: *mut NativeObject) -> *mut ArrayHeader {
    match native::get::<Net>(object) {
        Some(Net::Datagram(datagram)) => byte_array(&datagram.bytes),
        _ => byte_array(&[]),
    }
}

// ---------------------------------------------------------------------------
// Names and addresses
// ---------------------------------------------------------------------------

/// `resolve`: the IP addresses of a name, or why there are none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_resolve(host: *const Str, timeout_ms: i64) -> *mut NativeObject {
    let host = text(host);
    let found = waited(within(deadline(timeout_ms), async move {
        let addresses = lookup(&host, 0).await?;
        let mut ips: Vec<String> = Vec::new();
        for address in addresses {
            let ip = address.ip().to_string();
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        }
        Ok(Net::Names(ips))
    }));
    native::new(Net::from(found))
}

/// The addresses `resolve` found.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_names(object: *mut NativeObject) -> *mut ArrayHeader {
    match native::get::<Net>(object) {
        Some(Net::Names(names)) => super::http::string_array(names),
        _ => super::http::string_array(&[]),
    }
}

/// An IP address in its usual form, or "" when `text` is not one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_ip(text: *const Str) -> *mut Str {
    let ip = super::http::text(text).parse::<std::net::IpAddr>().map(|ip| ip.to_string());
    str_new(ip.unwrap_or_default().as_bytes())
}

/// `net.ip`: an IP address in its canonical form, an IPv4 in an IPv6 as
/// that IPv4, or "" when `text` is not one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_canonical_ip(text: *const Str) -> *mut Str {
    let ip = super::http::text(text).parse::<std::net::IpAddr>().map(|ip| ip.to_canonical().to_string());
    str_new(ip.unwrap_or_default().as_bytes())
}

/// What an IP address in its canonical form is, as bits: IPv4 1, IPv6 2,
/// loopback 4, private (RFC 1918 and RFC 4193) 8, unspecified 16.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_ip_kind(text: *const Str) -> i64 {
    let Ok(ip) = super::http::text(text).parse::<std::net::IpAddr>() else { return 0 };
    let private = match ip {
        std::net::IpAddr::V4(v4) => v4.is_private(),
        std::net::IpAddr::V6(v6) => v6.segments()[0] & 0xfe00 == 0xfc00,
    };
    [ip.is_ipv4(), ip.is_ipv6(), ip.is_loopback(), private, ip.is_unspecified()]
        .iter()
        .enumerate()
        .map(|(bit, &on)| (on as i64) << bit)
        .sum()
}

/// `net.network`: a network in CIDR notation, `10.0.0.0/8`, its address
/// cut to its prefix; an address alone is its own network; "" when `text`
/// is not one.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_network(text: *const Str) -> *mut Str {
    let text = super::http::text(text);
    let network = match text.parse::<ipnet::IpNet>() {
        Ok(network) => Some(network.trunc()),
        Err(_) => text.parse::<std::net::IpAddr>().ok().map(|ip| ipnet::IpNet::from(ip.to_canonical())),
    };
    str_new(network.map(|n| n.to_string()).unwrap_or_default().as_bytes())
}

/// Whether the network, as `network` gives it, holds the address.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_contains(network: *const Str, ip: *const Str) -> u8 {
    let (network, ip) = (super::http::text(network), super::http::text(ip));
    match (network.parse::<ipnet::IpNet>(), ip.parse::<std::net::IpAddr>()) {
        (Ok(network), Ok(ip)) => network.contains(&ip.to_canonical()) as u8,
        _ => 0,
    }
}

/// The local (`which` 0) or remote (1) address of a connection, the local
/// one of a listener or a socket, and who sent a datagram.
fn address_of(object: *mut NativeObject, which: i64) -> Option<Endpoint> {
    match native::get::<Net>(object)? {
        Net::Connection(connection) => Some(if which == 0 { &connection.local } else { &connection.remote }.clone()),
        Net::Listener(listener) => Some(listener.local.clone()),
        Net::Socket(socket) => Some(socket.local.clone()),
        Net::Datagram(datagram) => Some(datagram.from.clone()),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_address_ip(object: *mut NativeObject, which: i64) -> *mut Str {
    let ip = match address_of(object, which) {
        Some(Endpoint::Ip(address)) => address.ip().to_string(),
        _ => String::new(),
    };
    str_new(ip.as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_address_port(object: *mut NativeObject, which: i64) -> i64 {
    match address_of(object, which) {
        Some(Endpoint::Ip(address)) => address.port() as i64,
        _ => 0,
    }
}

/// The path of a Unix socket's address, or "".
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_address_path(object: *mut NativeObject, which: i64) -> *mut Str {
    match address_of(object, which) {
        Some(Endpoint::Path(path)) => str_new(path.as_bytes()),
        _ => str_new(b""),
    }
}

// ---------------------------------------------------------------------------
// Objects and failures
// ---------------------------------------------------------------------------

/// What `std.net`'s handles hold.
enum Net {
    Connection(Connection),
    Listener(Listener),
    Socket(Socket),
    Datagram(Datagram),
    Names(Vec<String>),
    Failed(Failure),
}

impl From<Result<Net, Failure>> for Net {
    fn from(result: Result<Net, Failure>) -> Net {
        result.unwrap_or_else(Net::Failed)
    }
}

/// The failure of an object (`which` 2), or of the last operation of a
/// connection's reading (0) or writing (1) direction, or of a socket's
/// `sendTo`: `(code, detail)`.
fn failure_of(object: *mut NativeObject, which: i64) -> Failure {
    match native::get::<Net>(object) {
        Some(Net::Failed(failure)) => failure.clone(),
        Some(Net::Connection(connection)) if connection.upgraded.load(std::sync::atomic::Ordering::Acquire) => {
            failure(CLOSED, "closed: `startTls` took it over")
        }
        Some(Net::Connection(connection)) if which < 2 => lock(direction(connection, which == 1)).clone(),
        Some(Net::Socket(socket)) if which < 2 => lock(&socket.failure).clone(),
        Some(_) => Failure::default(),
        None => failure(CLOSED, "closed"),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_failure(object: *mut NativeObject, which: i64) -> i64 {
    failure_of(object, which).code
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_detail(object: *mut NativeObject, which: i64) -> *mut Str {
    str_new(failure_of(object, which).detail.as_bytes())
}
