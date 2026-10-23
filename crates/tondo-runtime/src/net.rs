//! What `std.net` does in the runtime (standard library 14.4.6): TCP
//! connections and listeners, with TLS, UDP sockets and names, on the HTTP
//! runtime's tokio. A connection is two streams of `std.io` in
//! `Mode::Ready`, so a task waiting on the network holds neither a worker
//! nor a thread. Over TCP the task reads and writes the socket itself, and
//! a tokio task sends what the socket could not take at once; over TLS,
//! tokio tasks move the bytes between the stream and two buffers.

use super::http::{runtime, text};
use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, str_new};
use crate::io::{Host, Mode, Ready, StreamObject, byte_array, retain, run, run_until, wait_for};
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
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
struct Failure {
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

/// One direction of a connection: the bytes between the socket and its
/// stream, which the tokio task moving them and the stream's host share.
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

/// Where a connection's writes go: a TLS stream, or a TCP socket the
/// stream also writes itself.
enum Out {
    Tls(tokio::io::WriteHalf<Tls>),
    Tcp(Arc<OwnedWriteHalf>),
}

impl Out {
    async fn put(&mut self, data: &[u8]) -> std::io::Result<()> {
        match self {
            Out::Tls(stream) => {
                stream.write_all(data).await?;
                stream.flush().await
            }
            Out::Tcp(socket) => {
                let mut rest = data;
                while !rest.is_empty() {
                    socket.writable().await?;
                    match socket.try_write(rest) {
                        Ok(count) => rest = &rest[count..],
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            }
        }
    }
}

/// Sends what the stream leaves in `pipe`, and ends the direction after
/// the stream is done: TLS says so, and the last copy of a TCP socket's
/// writing half does when it goes.
async fn send(mut to: Out, pipe: Arc<Pipe>) {
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
        let sent = to.put(&chunk).await;
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
    if let Out::Tls(mut stream) = to {
        let _ = stream.shutdown().await;
    }
}

/// The reading half of a TCP connection as the host of a stream, which
/// reads the socket itself.
struct TcpIn {
    socket: Arc<OwnedReadHalf>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The reading half of a TLS connection as the host of a stream.
struct Inbox {
    pipe: Arc<Pipe>,
    waiting: bool,
    gave_up: Option<ErrorKind>,
}

/// The writing half of a connection as the host of a stream. Over TCP it
/// writes the socket itself while nothing waits in `pipe`.
struct Outbox {
    pipe: Arc<Pipe>,
    direct: Option<Arc<OwnedWriteHalf>>,
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

fn failed(flow: &Flow) -> std::io::Result<()> {
    flow.failure.as_ref().map_or(Ok(()), |failure| Err(std::io::Error::other(failure.clone())))
}

impl Host for TcpIn {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self.socket.try_read(buffer) {
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(would_block(&mut self.waiting, self.gave_up)),
            read => read,
        }
    }

    fn blocked(&mut self) -> Option<Ready> {
        let socket = self.socket.clone();
        std::mem::take(&mut self.waiting).then(|| {
            Box::pin(async move {
                let _ = socket.readable().await;
            }) as Ready
        })
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
            return Ok(count);
        }
        failed(&flow)?;
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

impl Host for Outbox {
    fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        let mut flow = self.pipe.flow();
        failed(&flow)?;
        let queued = flow.bytes.len() + flow.sending;
        let mut rest = data;
        if queued == 0
            && let Some(socket) = &self.direct
        {
            while !rest.is_empty() {
                match socket.try_write(rest) {
                    Ok(count) => rest = &rest[count..],
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => {
                        let failure = Failure::from(error);
                        flow.failure = Some(failure.clone());
                        return Err(std::io::Error::other(failure));
                    }
                }
            }
            if rest.is_empty() {
                return Ok(());
            }
        }
        if queued == 0 || queued + rest.len() <= HIGH {
            flow.bytes.extend_from_slice(rest);
            drop(flow);
            self.pipe.changed.notify_waiters();
            return Ok(());
        }
        Err(would_block(&mut self.waiting, self.gave_up))
    }

    /// Waits until the socket took everything.
    fn flush(&mut self) -> std::io::Result<()> {
        let flow = self.pipe.flow();
        failed(&flow)?;
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

/// A TCP connection: its streams, its addresses, and the last failure of
/// each direction.
struct Connection {
    reader: Held,
    writer: Held,
    local: SocketAddr,
    remote: SocketAddr,
    failures: [Mutex<Failure>; 2],
}

impl Connection {
    fn new(reader: Box<dyn Host>, writer: Box<dyn Host>, local: SocketAddr, remote: SocketAddr) -> Connection {
        Connection {
            reader: Held(crate::io::stream(reader, Mode::Ready, false)),
            writer: Held(crate::io::stream(writer, Mode::Ready, false)),
            local,
            remote,
            failures: Default::default(),
        }
    }
}

/// A connection over a TCP socket.
fn tcp(stream: tokio::net::TcpStream) -> Result<Connection, Failure> {
    let _ = stream.set_nodelay(true);
    let (local, remote) = (stream.local_addr()?, stream.peer_addr()?);
    let (from, to) = stream.into_split();
    let (to, outbox) = (Arc::new(to), Arc::new(Pipe::default()));
    runtime().spawn(send(Out::Tcp(to.clone()), outbox.clone()));
    let reader = TcpIn { socket: Arc::new(from), waiting: false, gave_up: None };
    let writer = Outbox { pipe: outbox, direct: Some(to), waiting: false, gave_up: None };
    Ok(Connection::new(Box::new(reader), Box::new(writer), local, remote))
}

/// A connection over TLS, and the pipes `secure` moves its bytes through
/// once the handshake is over.
fn tls(stream: &tokio::net::TcpStream) -> Result<(Connection, Arc<Pipe>, Arc<Pipe>), Failure> {
    let _ = stream.set_nodelay(true);
    let (local, remote) = (stream.local_addr()?, stream.peer_addr()?);
    let (inbox, outbox) = (Arc::new(Pipe::default()), Arc::new(Pipe::default()));
    let reader = Inbox { pipe: inbox.clone(), waiting: false, gave_up: None };
    let writer = Outbox { pipe: outbox.clone(), direct: None, waiting: false, gave_up: None };
    Ok((Connection::new(Box::new(reader), Box::new(writer), local, remote), inbox, outbox))
}

/// Starts the tasks that move the bytes of a TLS stream.
fn secure(stream: Tls, inbox: Arc<Pipe>, outbox: Arc<Pipe>) {
    let (from, to) = tokio::io::split(stream);
    runtime().spawn(receive(from, inbox));
    runtime().spawn(send(Out::Tls(to), outbox));
}

/// Connects to each address of `host` in turn, until one answers.
async fn open(host: &str, port: u16) -> Result<tokio::net::TcpStream, Failure> {
    let mut last = failure(NOT_FOUND, host);
    for address in lookup(host, port).await? {
        match tokio::net::TcpStream::connect(address).await {
            Ok(stream) => return Ok(stream),
            Err(error) => last = error.into(),
        }
    }
    Err(last)
}

/// `connect` and `connectTls` (`tls` 1): a connection, or why there is
/// none. `trust` names a PEM file of more certificates to trust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_connect(
    host: *const Str,
    port_number: i64,
    timeout_ms: i64,
    tls_on: u8,
    trust: *const Str,
) -> *mut NativeObject {
    let (host, trust) = (text(host), text(trust));
    let deadline = deadline(timeout_ms);
    let connected = port(port_number).and_then(|port| {
        let verify = match tls_on {
            0 => None,
            _ => {
                let mut config = super::http::tls(&trust).map_err(|problem| failure(TLS, problem))?;
                config.alpn_protocols.clear();
                let name = rustls_pki_types::ServerName::try_from(host.clone())
                    .map_err(|_| failure(INVALID_ADDRESS, format!("`{host}` is not a name TLS can verify")))?;
                Some((tokio_rustls::TlsConnector::from(Arc::new(config)), name))
            }
        };
        waited(within(deadline, async move {
            let stream = open(&host, port).await?;
            let Some((connector, name)) = verify else { return tcp(stream) };
            let (connection, inbox, outbox) = tls(&stream)?;
            let stream = connector.connect(name, stream).await.map_err(|error| {
                let mut why = Failure::from(&error).detail;
                if why.contains("UnknownIssuer") {
                    why.push_str("; if an authority of your own signed it, pass its PEM file as `trust:`");
                }
                failure(TLS, why)
            })?;
            secure(stream.into(), inbox, outbox);
            Ok(connection)
        }))
    });
    native::new(Net::from(connected.map(Net::Connection)))
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
        Some(Net::Connection(connection)) => Some(connection),
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
    listener: Mutex<Option<Arc<tokio::net::TcpListener>>>,
    tls: Option<tokio_rustls::TlsAcceptor>,
    local: SocketAddr,
    closed: Arc<tokio::sync::watch::Sender<bool>>,
}

/// A listener whose queue of connections not accepted yet is as long as
/// the system allows, so a burst of them waits instead of being dropped.
fn bind(address: SocketAddr) -> std::io::Result<tokio::net::TcpListener> {
    let socket = match address {
        SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4()?,
        SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6()?,
    };
    socket.set_reuseaddr(true)?;
    socket.bind(address)?;
    socket.listen(65_535)
}

/// `listen`: a listener on `host:port`, `0` for any free port, over TLS
/// when `cert` and `key` name PEM files.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_listen(
    host: *const Str,
    port_number: i64,
    cert: *const Str,
    key: *const Str,
) -> *mut NativeObject {
    let (host, cert, key) = (text(host), text(cert), text(key));
    let listening = port(port_number).and_then(|port| {
        let tls = match cert.is_empty() {
            true => None,
            false => Some(super::server::acceptor(&cert, &key, false).map_err(|problem| failure(TLS, problem))?),
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
        let local = listener.local_addr()?;
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        Ok(Net::Listener(Listener { listener: Mutex::new(Some(Arc::new(listener))), tls, local, closed }))
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
            let accepted = tokio::select! {
                accepted = socket.accept() => accepted,
                _ = closing.changed() => continue,
            };
            match accepted {
                Ok(accepted) => return Ok(accepted),
                Err(error) if keep == 0 => return Err(error.into()),
                // Out of descriptors, or a connection reset before it was
                // accepted: the next one may work.
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    }));
    let opened = accepted.and_then(|(stream, _)| {
        let Some(acceptor) = acceptor else { return tcp(stream).map(Net::Connection) };
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
        }
        Some(Net::Socket(socket)) => {
            lock(&socket.socket).take();
            socket.closed.send_replace(true);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// UDP
// ---------------------------------------------------------------------------

/// A UDP socket, until it is closed.
struct Socket {
    socket: Mutex<Option<Arc<tokio::net::UdpSocket>>>,
    local: SocketAddr,
    closed: Arc<tokio::sync::watch::Sender<bool>>,
    failure: Mutex<Failure>,
}

/// `bind`: a UDP socket on `host:port`, or why there is none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_bind(host: *const Str, port_number: i64) -> *mut NativeObject {
    let host = text(host);
    let bound = port(port_number).and_then(|port| {
        let socket = waited(async move {
            let mut last = failure(NOT_FOUND, host.as_str());
            for address in lookup(&host, port).await? {
                match tokio::net::UdpSocket::bind(address).await {
                    Ok(socket) => {
                        // macOS sends no datagram larger than the send
                        // buffer, 9 216 bytes unless raised.
                        let options = socket2::SockRef::from(&socket);
                        if options.send_buffer_size().is_ok_and(|size| size < 2 * DATAGRAM) {
                            let _ = options.set_send_buffer_size(2 * DATAGRAM);
                        }
                        return Ok(socket);
                    }
                    Err(error) => last = error.into(),
                }
            }
            Err(last)
        })?;
        let local = socket.local_addr()?;
        let closed = Arc::new(tokio::sync::watch::channel(false).0);
        let socket = Mutex::new(Some(Arc::new(socket)));
        Ok(Net::Socket(Socket { socket, local, closed, failure: Default::default() }))
    });
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
        let sending = sending.ok_or_else(|| failure(CLOSED, "closed"))?;
        if data.len() > DATAGRAM {
            let why = format!("a datagram of {} bytes; one holds at most {DATAGRAM}", data.len());
            return Err(failure(IO, why));
        }
        waited(async move { Ok(sending.send_to(&data, (address, port)).await?) })
    });
    let noted = sent.err().unwrap_or_default();
    let code = noted.code;
    *lock(&socket.failure) = noted;
    code
}

/// A datagram received, and who sent it.
struct Datagram {
    bytes: Vec<u8>,
    from: SocketAddr,
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
        if *closing.borrow() {
            return Err(failure(CLOSED, "closed"));
        }
        let mut buffer = vec![0; DATAGRAM];
        tokio::select! {
            received = receiving.recv_from(&mut buffer) => {
                let (count, from) = received?;
                buffer.truncate(count);
                Ok(Net::Datagram(Datagram { bytes: buffer, from }))
            }
            _ = closing.changed() => Err(failure(CLOSED, "closed")),
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

/// The local (`which` 0) or remote (1) address of a connection, the local
/// one of a listener or a socket, and who sent a datagram.
fn address_of(object: *mut NativeObject, which: i64) -> Option<SocketAddr> {
    match native::get::<Net>(object)? {
        Net::Connection(connection) => Some(if which == 0 { connection.local } else { connection.remote }),
        Net::Listener(listener) => Some(listener.local),
        Net::Socket(socket) => Some(socket.local),
        Net::Datagram(datagram) => Some(datagram.from),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_address_ip(object: *mut NativeObject, which: i64) -> *mut Str {
    str_new(address_of(object, which).map(|a| a.ip().to_string()).unwrap_or_default().as_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_net_address_port(object: *mut NativeObject, which: i64) -> i64 {
    address_of(object, which).map_or(0, |a| a.port() as i64)
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
