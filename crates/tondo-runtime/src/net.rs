//! What `std.net` does in the runtime (standard library 14.4.6): TCP
//! connections and listeners, with TLS, UDP sockets, Unix sockets of both
//! kinds and names. A connection
//! is two streams of `std.io` in `Mode::Ready`, so a task waiting on the
//! network holds neither a worker nor a thread. Sockets without TLS are the
//! workers' own (`poll`): a task reads and writes them itself, and what a
//! socket could not take at once goes when it can. Over TLS too: a read
//! decrypts and a write encrypts in the task's own operation, after a
//! handshake of its own. Names are looked up with the HTTP runtime's tokio.

use super::http::text;
use super::native::{self, NativeObject};
use super::poll::{Direction, Registered};
use super::{ArrayHeader, Str, str_new};
use crate::array_items;
use crate::byte_array;
use crate::io::{Host, Mode, Ready, StreamObject, retain, run, run_until, wait_for};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

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
    let found = addresses(host, port).await.map_err(|_| failure(NOT_FOUND, host))?;
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

/// The addresses `host` resolves to: in a recorded run, and its replay,
/// looked up on the worker, which the recording has the calls of, and not
/// on a thread of tokio's.
pub(crate) async fn addresses(host: &str, port: u16) -> std::io::Result<Vec<SocketAddr>> {
    if super::replay::on() {
        return std::net::ToSocketAddrs::to_socket_addrs(&(host, port)).map(Iterator::collect);
    }
    tokio::net::lookup_host((host, port)).await.map(Iterator::collect)
}

fn port(port: i64) -> Result<u16, Failure> {
    u16::try_from(port).map_err(|_| failure(INVALID_ADDRESS, format!("port {port} is not between 0 and 65535")))
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

/// A TLS connection whose streams read and write it in place, as TCP's do:
/// a read decrypts what the socket has, and a write encrypts and sends what
/// the socket takes, in the task's own operation. The handshake comes
/// first, on its own.
struct Secure {
    socket: Registered<mio::net::TcpStream>,
    session: Mutex<Session>,
}

struct Session {
    tls: rustls::Connection,
    /// Until the handshake is over: the wakers of the streams that wait for
    /// it.
    shaking: Option<Vec<Waker>>,
    /// Why reading, and writing, fail from now on.
    failures: [Option<Failure>; 2],
    /// Bytes written since TLS last sent all it held.
    unsent: usize,
    /// The writing stream let go with bytes to send: the connection ends its
    /// writing after them, and holds itself until then.
    keep: Option<Arc<Secure>>,
}

/// The socket, as rustls reads and writes it.
struct Raw<'a>(&'a Registered<mio::net::TcpStream>);

impl Read for Raw<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.attempt(Direction::Read, |socket| Wire::take(socket, buffer))
    }
}

impl Write for Raw<'_> {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.attempt(Direction::Write, |socket| socket.give(data))
    }

    fn write_vectored(&mut self, parts: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        self.0.attempt(Direction::Write, |socket| socket.give_all(parts))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Secure {
    fn new(socket: Registered<mio::net::TcpStream>, mut tls: rustls::Connection) -> Arc<Secure> {
        let _ = socket.io().set_nodelay(true);
        // A write takes all it is given, as TCP's does, which `HIGH` bounds:
        // with rustls's own limit, a write the socket could not take at once
        // would take less.
        tls.set_buffer_limit(None);
        let session = Session { tls, shaking: Some(Vec::new()), failures: [None, None], unsent: 0, keep: None };
        let secure = Arc::new(Secure { socket, session: Mutex::new(session) });
        let weak = Arc::downgrade(&secure);
        secure.socket.on_writable(Arc::new(move || {
            if let Some(secure) = weak.upgrade() {
                secure.writable();
            }
        }));
        secure
    }

    /// Sends what TLS holds while the socket takes it; the rest goes when it
    /// can take more.
    fn push(&self, session: &mut Session) -> std::io::Result<()> {
        while session.tls.wants_write() {
            match session.tls.write_tls(&mut Raw(&self.socket)) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        session.unsent = 0;
        Ok(())
    }

    /// `push`, noting a failure, which every write after says.
    fn sent(&self, session: &mut Session) -> std::io::Result<()> {
        self.push(session).map_err(|error| {
            let failure = Failure::from(error);
            session.failures[1] = Some(failure.clone());
            std::io::Error::other(failure)
        })
    }

    /// Takes in the records read; the alert of a failure goes out first.
    fn process(&self, session: &mut Session) -> std::io::Result<()> {
        match session.tls.process_new_packets() {
            Ok(_) => Ok(()),
            Err(error) => {
                let _ = self.push(session);
                Err(std::io::Error::new(ErrorKind::InvalidData, error))
            }
        }
    }

    /// One step of the handshake: `None` once it is over, or the direction
    /// of the socket it waits for.
    fn shake(&self, session: &mut Session) -> std::io::Result<Option<Direction>> {
        loop {
            self.push(session)?;
            if session.tls.wants_write() {
                return Ok(Some(Direction::Write));
            }
            if !session.tls.is_handshaking() {
                return Ok(None);
            }
            match session.tls.read_tls(&mut Raw(&self.socket)) {
                Ok(0) => {
                    let why = "the other side ended the connection in the TLS handshake";
                    return Err(std::io::Error::new(ErrorKind::UnexpectedEof, why));
                }
                Ok(_) => self.process(session)?,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(Some(Direction::Read)),
                Err(error) => return Err(error),
            }
        }
    }

    /// The handshake is over, or failed: whoever waits for it goes on.
    fn shaken(&self, outcome: &Result<(), Failure>) {
        let waiting = {
            let mut session = lock(&self.session);
            if let Err(failure) = outcome {
                session.failures = [Some(failure.clone()), Some(failure.clone())];
            }
            session.shaking.take()
        };
        waiting.into_iter().flatten().for_each(Waker::wake);
    }

    /// When the socket can take more: sends what TLS holds, and ends the
    /// writing of a connection whose stream let go once nothing is left.
    fn writable(&self) {
        let mut session = lock(&self.session);
        let _ = self.sent(&mut session);
        if session.keep.is_some() && (!session.tls.wants_write() || session.failures[1].is_some()) {
            let _ = self.socket.io().end();
            let keep = session.keep.take();
            drop(session);
            drop(keep);
        }
    }

    /// What a stream waits for, as a future.
    fn ready(self: &Arc<Self>, awaited: Awaited) -> Ready {
        if let Awaited::Socket(direction) = awaited {
            return Ready::Socket(self.socket.ready(direction));
        }
        let secure = self.clone();
        Ready::Other(Box::pin(std::future::poll_fn(move |context| {
            let mut session = lock(&secure.session);
            let Some(waiting) = session.shaking.as_mut() else { return Poll::Ready(()) };
            if !waiting.iter().any(|waker| waker.will_wake(context.waker())) {
                waiting.push(context.waker().clone());
            }
            Poll::Pending
        })))
    }
}

/// The handshake of a connection: in the task that connects, or at the
/// workers' turns for one accepted.
async fn handshake(secure: &Secure) -> Result<(), Failure> {
    loop {
        let step = secure.shake(&mut lock(&secure.session));
        match step {
            Ok(None) => return Ok(()),
            Ok(Some(direction)) => secure.socket.ready(direction).await,
            Err(error) => return Err(error.into()),
        }
    }
}

/// What a stream of a TLS connection waits for.
#[derive(Clone, Copy)]
enum Awaited {
    Socket(Direction),
    Handshake,
}

/// A connected socket of the workers' poll: TCP or Unix.
pub(crate) trait Wire: mio::event::Source + Send + Sync + 'static {
    fn take(&self, buffer: &mut [u8]) -> std::io::Result<usize>;
    fn give(&self, data: &[u8]) -> std::io::Result<usize>;
    fn give_all(&self, parts: &[std::io::IoSlice<'_>]) -> std::io::Result<usize>;
    fn end(&self) -> std::io::Result<()>;
}

impl Wire for mio::net::TcpStream {
    fn take(&self, buffer: &mut [u8]) -> std::io::Result<usize> {
        (&*self).read(buffer)
    }
    fn give(&self, data: &[u8]) -> std::io::Result<usize> {
        (&*self).write(data)
    }
    fn give_all(&self, parts: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        (&*self).write_vectored(parts)
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
    fn give_all(&self, parts: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        (&*self).write_vectored(parts)
    }
    fn end(&self) -> std::io::Result<()> {
        self.shutdown(Shutdown::Write)
    }
}

/// A socket of the workers' poller as tokio's traits read and write it,
/// which hyper and TLS take: an operation that would block leaves the waker
/// it was polled with to the poller, which wakes it on the socket's event.
pub(crate) struct Polled<S: Wire> {
    socket: Registered<S>,
}

impl<S: Wire> Polled<S> {
    pub(crate) fn new(io: S) -> std::io::Result<Polled<S>> {
        Ok(Polled { socket: Registered::new(io)? })
    }
}

impl<S: Wire> From<Registered<S>> for Polled<S> {
    fn from(socket: Registered<S>) -> Polled<S> {
        Polled { socket }
    }
}

impl<S: Wire> tokio::io::AsyncRead for Polled<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // As tokio does: the socket writes the bytes it reads, and reads none.
        let unfilled = unsafe { &mut *(buffer.unfilled_mut() as *mut [std::mem::MaybeUninit<u8>] as *mut [u8]) };
        let read = self.socket.poll_attempt(context, Direction::Read, |socket| socket.take(unfilled));
        read.map_ok(|count| {
            unsafe { buffer.assume_init(count) };
            buffer.advance(count);
        })
    }
}

impl<S: Wire> tokio::io::AsyncWrite for Polled<S> {
    fn poll_write(self: Pin<&mut Self>, context: &mut Context<'_>, data: &[u8]) -> Poll<std::io::Result<usize>> {
        self.socket.poll_attempt(context, Direction::Write, |socket| socket.give(data))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        parts: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.socket.poll_attempt(context, Direction::Write, |socket| socket.give_all(parts))
    }

    fn is_write_vectored(&self) -> bool {
        true
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(self.socket.io().end())
    }
}

/// A socket of the workers' poll, and what its writing direction holds.
struct Tcp<S: Wire> {
    socket: Registered<S>,
    unsent: Mutex<Unsent<S>>,
    /// Whether `startTls` took the socket over: then letting go of it does
    /// not end its writing, which TLS goes on with.
    upgraded: Arc<std::sync::atomic::AtomicBool>,
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
            if !self.upgraded.load(std::sync::atomic::Ordering::Acquire) {
                let _ = self.socket.io().end();
            }
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

/// The reading half of a TLS connection as the host of a stream, which
/// decrypts what the socket has.
struct SecureIn {
    secure: Arc<Secure>,
    waiting: Option<Awaited>,
    gave_up: Option<ErrorKind>,
}

/// The writing half of a TLS connection as the host of a stream, which
/// encrypts what it writes and sends what the socket takes.
struct SecureOut {
    secure: Arc<Secure>,
    waiting: Option<Awaited>,
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

/// `would_block` for a stream of a TLS connection, which notes what it waits
/// for.
fn would_wait(waiting: &mut Option<Awaited>, gave_up: Option<ErrorKind>, awaited: Awaited) -> std::io::Error {
    gave_up
        .unwrap_or_else(|| {
            *waiting = Some(awaited);
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
        std::mem::take(&mut self.waiting).then(|| Ready::Socket(self.tcp.socket.ready(Direction::Read)))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl Host for SecureIn {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let secure = &self.secure;
        let mut session = lock(&secure.session);
        if session.shaking.is_some() {
            return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Handshake));
        }
        failed(&session.failures[0])?;
        loop {
            match session.tls.reader().read(buffer) {
                Ok(count) => {
                    super::diagnostics::NET_READ.add(count as i64);
                    return Ok(count);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
            match session.tls.read_tls(&mut Raw(&secure.socket)) {
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Socket(Direction::Read)));
                }
                Err(error) => return Err(error),
            }
            secure.process(&mut session)?;
            // What TLS answers, as to a key update, goes out.
            let _ = secure.sent(&mut session);
        }
    }

    fn blocked(&mut self) -> Option<Ready> {
        self.waiting.take().map(|awaited| self.secure.ready(awaited))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
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
        std::mem::take(&mut self.waiting).then(|| Ready::Socket(self.tcp.socket.ready(Direction::Write)))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

impl<S: Wire> Drop for TcpOut<S> {
    fn drop(&mut self) {
        let mut unsent = lock(&self.tcp.unsent);
        if self.tcp.upgraded.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        if unsent.bytes.is_empty() || unsent.failure.is_some() {
            let _ = self.tcp.socket.io().end();
        } else {
            unsent.keep = Some(self.tcp.clone());
        }
    }
}

impl Host for SecureOut {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let secure = &self.secure;
        let mut session = lock(&secure.session);
        if session.shaking.is_some() {
            return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Handshake));
        }
        failed(&session.failures[1])?;
        secure.sent(&mut session)?;
        if session.unsent > 0 && session.unsent + data.len() > HIGH {
            return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Socket(Direction::Write)));
        }
        session.tls.writer().write_all(data)?;
        super::diagnostics::NET_WRITTEN.add(data.len() as i64);
        session.unsent += data.len();
        secure.sent(&mut session)
    }

    /// Waits until the socket took everything.
    fn flush(&mut self) -> std::io::Result<()> {
        let secure = &self.secure;
        let mut session = lock(&secure.session);
        if session.shaking.is_some() {
            return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Handshake));
        }
        failed(&session.failures[1])?;
        secure.sent(&mut session)?;
        if session.tls.wants_write() {
            return Err(would_wait(&mut self.waiting, self.gave_up, Awaited::Socket(Direction::Write)));
        }
        Ok(())
    }

    fn blocked(&mut self) -> Option<Ready> {
        self.waiting.take().map(|awaited| self.secure.ready(awaited))
    }

    fn give_up(&mut self, kind: Option<ErrorKind>) {
        self.gave_up = kind;
    }
}

/// Letting go ends the writing, after what TLS holds and its close.
impl Drop for SecureOut {
    fn drop(&mut self) {
        let mut session = lock(&self.secure.session);
        if session.shaking.is_none() {
            session.tls.send_close_notify();
            let _ = self.secure.sent(&mut session);
        }
        if session.shaking.is_some() || !session.tls.wants_write() || session.failures[1].is_some() {
            let _ = self.secure.socket.io().end();
        } else {
            session.keep = Some(self.secure.clone());
        }
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
    upgraded: Arc<std::sync::atomic::AtomicBool>,
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
    let upgraded = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let tcp = Arc::new(Tcp { socket, unsent: Mutex::new(Unsent::default()), upgraded: upgraded.clone() });
    let weak = Arc::downgrade(&tcp);
    tcp.socket.on_writable(Arc::new(move || {
        if let Some(tcp) = weak.upgrade() {
            tcp.writable();
        }
    }));
    let reader = TcpIn { tcp: tcp.clone(), waiting: false, gave_up: None };
    let writer = TcpOut { tcp, waiting: false, gave_up: None };
    Connection { upgraded, ..Connection::new(Box::new(reader), Box::new(writer), local, remote) }
}

/// A connection over TLS, from its handshake on.
fn secured(secure: Arc<Secure>) -> Result<Connection, Failure> {
    let (local, remote) = (secure.socket.io().local_addr()?, secure.socket.io().peer_addr()?);
    let reader = SecureIn { secure: secure.clone(), waiting: None, gave_up: None };
    let writer = SecureOut { secure, waiting: None, gave_up: None };
    Ok(Connection::new(Box::new(reader), Box::new(writer), local.into(), remote.into()))
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
pub(crate) async fn connect(address: SocketAddr) -> std::io::Result<Registered<mio::net::TcpStream>> {
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

/// What verifies a TLS connection to `host`: the roots, the authorities
/// of `trust`, and the certificate and key it shows, as `http::tls` takes
/// them.
fn verifier(
    host: &str,
    trust: &str,
    cert: &str,
    key: &str,
    minimum: i64,
) -> Result<(Arc<rustls::ClientConfig>, rustls_pki_types::ServerName<'static>), Failure> {
    let mut config = super::http::tls(trust, cert, key, minimum).map_err(|problem| failure(TLS, problem))?;
    config.alpn_protocols.clear();
    let name = rustls_pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| failure(INVALID_ADDRESS, format!("`{host}` is not a name TLS can verify")))?;
    Ok((Arc::new(config), name))
}

/// The handshake of a client over `socket`, and its connection.
async fn client(
    socket: Registered<mio::net::TcpStream>,
    config: Arc<rustls::ClientConfig>,
    name: rustls_pki_types::ServerName<'static>,
) -> Result<Connection, Failure> {
    let tls = rustls::ClientConnection::new(config, name).map_err(|error| failure(TLS, error.to_string()))?;
    let secure = Secure::new(socket, tls.into());
    let shaken = handshake(&secure).await.map_err(|error| {
        let mut why = error.detail;
        if why.contains("UnknownIssuer") {
            why.push_str("; if an authority of your own signed it, pass its PEM as `trust:`");
        }
        failure(TLS, why)
    });
    secure.shaken(&shaken);
    shaken?;
    secured(secure)
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
            let Some((config, name)) = verify else { return tcp(open(&host, port, connect).await?) };
            client(open(&host, port, connect).await?, config, name).await
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
    let upgraded = verifier(&host, &text(trust), &text(cert), &text(key), minimum).and_then(|(config, name)| {
        // The socket goes on under a descriptor of its own; the plain
        // connection closes its own without ending the socket.
        let duplicate = unsafe { libc::dup(fd) };
        if duplicate < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        connection.upgraded.store(true, std::sync::atomic::Ordering::Release);
        let socket = unsafe { std::net::TcpStream::from_raw_fd(duplicate) };
        socket.set_nonblocking(true)?;
        let socket = Registered::new(mio::net::TcpStream::from_std(socket))?;
        waited(within(deadline(timeout_ms), client(socket, config, name)))
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
    op: impl FnMut(&mut crate::io::StreamState) -> std::io::Result<R> + Send,
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
    let (bytes, read) = match on(&connection.reader, deadline(timeout_ms), |s| s.read(max)) {
        Ok(Some(bytes)) => (bytes, Ok(true)),
        Ok(None) => (Vec::new(), Ok(false)),
        Err(failure) => (Vec::new(), Err(failure)),
    };
    note(connection, false, read);
    byte_array(&bytes)
}

/// `readLine`: the next line without its `\n` or `\r\n`, or none at the end.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_read_line(object: *mut NativeObject, timeout_ms: i64) -> *mut Str {
    let Some(connection) = connection_of(object) else { return str_new(b"") };
    let (line, read) = match on(&connection.reader, deadline(timeout_ms), |s| s.read_line(crate::io::made_text)) {
        Ok(Some(Some(line))) => (line.0, Ok(true)),
        Ok(Some(None)) => (str_new(b""), Err(failure(IO, "a line that is not UTF-8"))),
        Ok(None) => (str_new(b""), Ok(false)),
        Err(failure) => (str_new(b""), Err(failure)),
    };
    note(connection, false, read);
    line
}

/// `readAll`: everything until the other side ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_read_all(object: *mut NativeObject, timeout_ms: i64) -> *mut ArrayHeader {
    let Some(connection) = connection_of(object) else { return byte_array(&[]) };
    let (bytes, read) = match on(&connection.reader, deadline(timeout_ms), |s| s.read_rest()) {
        Ok(bytes) => (bytes, Ok(true)),
        Err(failure) => (Vec::new(), Err(failure)),
    };
    note(connection, false, read);
    byte_array(&bytes)
}

fn write(object: *mut NativeObject, data: &[u8], timeout_ms: i64) -> i64 {
    let Some(connection) = connection_of(object) else { return CLOSED };
    let written = on(&connection.writer, deadline(timeout_ms), |s| s.write(data));
    note(connection, true, written.map(|_| true))
}

/// `write`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_write(object: *mut NativeObject, text: *const Str, timeout_ms: i64) -> i64 {
    write(object, unsafe { super::str_bytes(text) }, timeout_ms)
}

/// `writeBytes`: 0, or the code of a failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_write_bytes(
    object: *mut NativeObject,
    data: *mut ArrayHeader,
    timeout_ms: i64,
) -> i64 {
    write(object, unsafe { array_items::<u8>(data) }, timeout_ms)
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

/// A listener, until it is closed, and the TLS of its connections when it
/// has one.
struct Listener {
    listener: Mutex<Option<Acceptor>>,
    tls: Option<Arc<rustls::ServerConfig>>,
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
                    .map_err(|problem| failure(TLS, problem))?
                    .config()
                    .clone(),
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
    let (config, closed) = (listener.tls.clone(), listener.closed.clone());
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
        let Some(config) = config else { return tcp(Registered::new(stream)?).map(Net::Connection) };
        let tls = rustls::ServerConnection::new(config).map_err(|error| failure(TLS, error.to_string()))?;
        let secure = Secure::new(Registered::new(stream)?, tls.into());
        let connection = secured(secure.clone())?;
        // The handshake happens on its own, at the workers' turns, so a
        // slow client does not hold up the next one; its failure is the
        // first read's and write's.
        let _ = super::server::detach(async move {
            let shaken = handshake(&secure).await.map_err(|why| failure(TLS, why.detail));
            secure.shaken(&shaken);
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
    let data = unsafe { array_items::<u8>(data) }.to_vec();
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
    let (data, path) = (unsafe { array_items::<u8>(data) }.to_vec(), text(path));
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
        Some(Net::Names(names)) => crate::string_array(names),
        _ => crate::string_array(&[]),
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
