//! Bounded Rustls state driven through nonblocking TCP without a TLS worker.

use rustls::ClientConnection;
use std::{
    io::{self, Read, Write},
    sync::{Arc, Mutex},
};
use tokio::io::Interest;
use tondo_stdlib::{
    io::ReadResult,
    net::{HostName, NetError, NetLimits, TlsError},
};

use super::{TlsConfig, io_error, tls_error, transport::TcpStream};

struct Session {
    connection: ClientConnection,
    eof: bool,
    writer_closed: bool,
    failure: Option<TlsError>,
}

struct Transport {
    tcp: TcpStream,
    session: Mutex<Session>,
}

pub(crate) struct TlsStream(Arc<Transport>);
pub(crate) struct TlsReadHalf(Arc<Transport>);
pub(crate) struct TlsWriteHalf(Arc<Transport>);

struct SocketReader<'a>(&'a TcpStream);
struct SocketWriter<'a>(&'a TcpStream);

impl Read for SocketReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.0.socket().try_read(bytes)
    }
}

impl Write for SocketWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.socket().try_write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Transport {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Session>, TlsError> {
        self.session
            .lock()
            .map_err(|_| TlsError::Transport(NetError::Host))
    }

    /// State transitions and individual socket syscalls are serialized. No
    /// mutex is held across await, so a reader cannot prevent its writer from
    /// progressing a shared TLS session.
    fn progress(&self, session: &mut Session) -> Result<(), TlsError> {
        if let Some(error) = session.failure {
            return Err(error);
        }
        if session.connection.wants_write() {
            match session.connection.write_tls(&mut SocketWriter(&self.tcp)) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(TlsError::Transport(io_error(&error))),
            }
        }
        if !session.eof && session.connection.wants_read() {
            match session.connection.read_tls(&mut SocketReader(&self.tcp)) {
                Ok(length) => {
                    session.eof = length == 0;
                    session
                        .connection
                        .process_new_packets()
                        .map_err(tls_error)?;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(TlsError::Transport(io_error(&error))),
            }
        }
        Ok(())
    }

    async fn readiness(&self, writing: bool) -> Result<(), TlsError> {
        let interest = if writing {
            Interest::READABLE | Interest::WRITABLE
        } else {
            Interest::READABLE
        };
        self.tcp
            .socket()
            .ready(interest)
            .await
            .map_err(|error| TlsError::Transport(io_error(&error)))?;
        Ok(())
    }
}

impl TlsStream {
    /// Own the TCP transport from entry. Any validation/handshake error or
    /// cancellation drops it; no partially usable plaintext stream escapes.
    pub(crate) async fn connect(
        tcp: TcpStream,
        host: HostName,
        config: &TlsConfig,
    ) -> Result<Self, TlsError> {
        let connection = config.connection(host)?;
        let transport = Arc::new(Transport {
            tcp,
            session: Mutex::new(Session {
                connection,
                eof: false,
                writer_closed: false,
                failure: None,
            }),
        });
        loop {
            let writing = {
                let mut session = transport.lock()?;
                transport.progress(&mut session)?;
                if !session.connection.is_handshaking() && !session.connection.wants_write() {
                    return Ok(Self(transport.clone()));
                }
                if session.eof {
                    return Err(TlsError::HandshakeFailed);
                }
                session.connection.wants_write()
            };
            transport.readiness(writing).await?;
        }
    }

    pub(crate) fn split(self) -> (TlsReadHalf, TlsWriteHalf) {
        (TlsReadHalf(self.0.clone()), TlsWriteHalf(self.0))
    }
}

impl TlsReadHalf {
    pub(crate) async fn read(&self, max: i128, limits: NetLimits) -> Result<ReadResult, TlsError> {
        let size = limits.read_request(max).map_err(|error| match error {
            NetError::ResourceLimit => TlsError::ResourceLimit,
            error => TlsError::Transport(error),
        })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| TlsError::ResourceLimit)?;
        bytes.resize(size, 0);
        loop {
            let writing = {
                let mut session = self.0.lock()?;
                if let Some(error) = session.failure {
                    return Err(error);
                }
                // Drain validated plaintext before requesting more ciphertext.
                match session.connection.reader().read(&mut bytes) {
                    Ok(length) => {
                        bytes.truncate(length);
                        return Ok(if length == 0 {
                            ReadResult::Eof
                        } else {
                            ReadResult::Data(bytes)
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(TlsError::Transport(io_error(&error))),
                }
                self.0.progress(&mut session)?;
                // New plaintext or EOF may have arrived in this transition.
                match session.connection.reader().read(&mut bytes) {
                    Ok(length) => {
                        bytes.truncate(length);
                        return Ok(if length == 0 {
                            ReadResult::Eof
                        } else {
                            ReadResult::Data(bytes)
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(TlsError::Transport(io_error(&error))),
                }
                session.connection.wants_write()
            };
            self.0.readiness(writing).await?;
        }
    }
}

impl TlsWriteHalf {
    /// Report exactly the plaintext accepted into the bounded Rustls buffer.
    /// A subsequent transport failure affects flush/the next operation, never
    /// changes already committed progress into an unknown-partial error.
    pub(crate) fn write(&self, bytes: &[u8]) -> Result<usize, TlsError> {
        let mut session = self.0.lock()?;
        if session.writer_closed {
            return Err(TlsError::Closed);
        }
        self.0.progress(&mut session)?;
        let length = session
            .connection
            .writer()
            .write(bytes)
            .map_err(|error| TlsError::Transport(io_error(&error)))?;
        if !bytes.is_empty() && length == 0 {
            return Err(TlsError::ResourceLimit);
        }
        if let Err(error) = self.0.progress(&mut session) {
            session.failure = Some(error);
        }
        Ok(length)
    }

    pub(crate) async fn flush(&self) -> Result<(), TlsError> {
        loop {
            {
                let mut session = self.0.lock()?;
                self.0.progress(&mut session)?;
                if !session.connection.wants_write() {
                    return Ok(());
                }
            }
            self.0.readiness(true).await?;
        }
    }

    pub(crate) async fn shutdown(&self) -> Result<(), TlsError> {
        {
            let mut session = self.0.lock()?;
            if !session.writer_closed {
                session.connection.send_close_notify();
                session.writer_closed = true;
            }
        }
        self.flush().await
    }
}
