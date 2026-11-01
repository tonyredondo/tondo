//! The transport of `std.grpc`'s clients (standard library 14.41): a
//! channel to a server keeps one HTTP/2 connection, over TLS with ALPN or
//! as h2c, which its calls share; a call sends its messages framed as gRPC
//! frames them, as they are given, and receives the server's messages,
//! headers and status, which its task waits for without holding a worker.

use crate::io::{array_bytes, byte_array, wait_for};
use crate::native::{self, NativeObject};
use crate::{ArrayHeader, Str, str_bytes, str_new};
use bytes::{Buf, Bytes, BytesMut};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// The codes of gRPC's status that the transport gives itself.
const CANCELLED: i64 = 1;
const RESOURCE_EXHAUSTED: i64 = 8;
const INTERNAL: i64 = 13;
const UNAVAILABLE: i64 = 14;
const UNKNOWN: i64 = 2;

fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------

/// How a channel spreads its calls over its target's addresses: all on the
/// first that answers, or each on the next of those that answer.
#[derive(Clone, Copy, PartialEq)]
enum Balancing {
    PickFirst,
    RoundRobin,
}

/// gRPC's backoff between attempts to connect to an address, as its
/// connection-backoff document says: 1 s, then 1.6 times longer each time
/// up to 2 minutes, give or take a fifth, and 20 s for each attempt.
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Where a channel connects: the hosts of its target with their ports,
/// and what it knows of the addresses they resolve to.
struct Channel {
    secure: bool,
    hosts: Vec<(String, u16)>,
    /// The target without its scheme, for messages.
    target: String,
    tls: Option<Arc<rustls::ClientConfig>>,
    balancing: Balancing,
    pool: tokio::sync::Mutex<Pool>,
}

/// The addresses of a channel, and how many calls round robin gave out.
struct Pool {
    backends: Vec<Backend>,
    /// Whether to resolve the hosts again before the next call: at first,
    /// and once a connection dropped or could not be made.
    stale: bool,
    given: usize,
}

/// An address of a host of the target: its connection, and when it may be
/// tried again after an attempt failed.
struct Backend {
    host: String,
    port: u16,
    address: SocketAddr,
    sender: Option<SendRequest<Body>>,
    connecting: bool,
    retry: Option<Instant>,
    backoff: Duration,
}

impl Backend {
    /// `host:port`, as `:authority` and messages give it.
    fn authority(&self) -> String {
        match self.host.contains(':') {
            true => format!("[{}]:{}", self.host, self.port),
            false => format!("{}:{}", self.host, self.port),
        }
    }

    /// Whether a call may try to connect to it now.
    fn idle(&self, now: Instant) -> bool {
        self.sender.is_none() && !self.connecting && self.retry.is_none_or(|at| at <= now)
    }

    /// Keeps what an attempt to connect gave: its connection, or when to
    /// try again, and why it failed.
    fn settle(&mut self, outcome: Result<SendRequest<Body>, String>) -> Option<String> {
        self.connecting = false;
        match outcome {
            Ok(sender) => {
                (self.sender, self.retry, self.backoff) = (Some(sender), None, FIRST_BACKOFF);
                None
            }
            Err(problem) => {
                let bytes = crate::codec::entropy(2);
                let random = u16::from_le_bytes([bytes[0], bytes[1]]);
                let jitter = 0.8 + 0.4 * f64::from(random) / f64::from(u16::MAX);
                self.retry = Some(Instant::now() + self.backoff.mul_f64(jitter));
                self.backoff = self.backoff.mul_f64(1.6).min(MAX_BACKOFF);
                Some(problem)
            }
        }
    }
}

/// An HTTP/2 connection to an address, over TLS with the host's name when
/// the channel has TLS.
async fn connect(
    tls: Option<Arc<rustls::ClientConfig>>,
    host: String,
    address: SocketAddr,
    authority: String,
) -> Result<SendRequest<Body>, String> {
    let attempt = async {
        let tcp = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|error| format!("cannot connect to {authority}: {error}"))?;
        let _ = tcp.set_nodelay(true);
        let Some(config) = tls else { return handshake(tcp, &authority).await };
        let name = rustls_pki_types::ServerName::try_from(host.clone())
            .map_err(|_| format!("`{host}` is not a name TLS checks"))?;
        let stream = tokio_rustls::TlsConnector::from(config)
            .connect(name, tcp)
            .await
            .map_err(|error| format!("TLS with {authority} failed: {error}"))?;
        handshake(stream, &authority).await
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, attempt).await {
        Ok(outcome) => outcome,
        Err(_) => Err(format!("{authority} did not answer in {} s", CONNECT_TIMEOUT.as_secs())),
    }
}

async fn handshake<S>(stream: S, authority: &str) -> Result<SendRequest<Body>, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (sender, connection) = hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
        .await
        .map_err(|error| format!("HTTP/2 with {authority} failed: {error}"))?;
    tokio::spawn(connection);
    Ok(sender)
}

impl Channel {
    /// A connection for a call, and its `host:port`, as the channel's
    /// balancing picks it: with none, it connects to the addresses that
    /// may be tried, all at once for round robin and in order for pick
    /// first; an address whose attempt failed waits for its backoff.
    async fn pick(self: &Arc<Self>) -> Result<(SendRequest<Body>, String), String> {
        let mut guard = self.pool.lock().await;
        let pool = &mut *guard;
        for backend in &mut pool.backends {
            if backend.sender.as_ref().is_some_and(SendRequest::is_closed) {
                backend.sender = None;
                pool.stale = true;
            }
        }
        if pool.stale {
            self.resolve(pool).await?;
        }
        let now = Instant::now();
        let mut problem = None;
        if pool.backends.iter().all(|b| b.sender.is_none()) {
            let idle: Vec<usize> = (0..pool.backends.len()).filter(|&i| pool.backends[i].idle(now)).collect();
            let attempt = |b: &Backend| connect(self.tls.clone(), b.host.clone(), b.address, b.authority());
            match self.balancing {
                Balancing::PickFirst => {
                    for at in idle {
                        let outcome = attempt(&pool.backends[at]).await;
                        problem = pool.backends[at].settle(outcome);
                        if problem.is_none() {
                            break;
                        }
                    }
                }
                Balancing::RoundRobin => {
                    let outcomes = futures_util::future::join_all(idle.iter().map(|&at| attempt(&pool.backends[at])));
                    for (at, outcome) in idle.iter().zip(outcomes.await) {
                        problem = pool.backends[*at].settle(outcome).or(problem);
                    }
                }
            }
            pool.stale |= problem.is_some();
        }
        let ready: Vec<usize> = (0..pool.backends.len()).filter(|&i| pool.backends[i].sender.is_some()).collect();
        let chosen = match (self.balancing, ready.len()) {
            (_, 0) => return Err(problem.unwrap_or_else(|| self.waiting(pool, now))),
            (Balancing::PickFirst, _) => ready[0],
            (Balancing::RoundRobin, count) => {
                pool.given = pool.given.wrapping_add(1);
                self.reconnect(pool, now);
                ready[(pool.given - 1) % count]
            }
        };
        let backend = &pool.backends[chosen];
        Ok((
            backend.sender.clone().unwrap_or_else(|| unreachable!("a ready address has a connection")),
            backend.authority(),
        ))
    }

    /// Why no address answers while each waits for its backoff.
    fn waiting(&self, pool: &Pool, now: Instant) -> String {
        let next = pool.backends.iter().filter_map(|b| b.retry).min().map_or(0.0, |at| (at - now).as_secs_f64());
        format!("no address of {} answers; the channel tries again in {next:.1} s", self.target)
    }

    /// Connects round robin's other addresses in the background, so that
    /// the calls that follow have them.
    fn reconnect(self: &Arc<Self>, pool: &mut Pool, now: Instant) {
        for backend in pool.backends.iter_mut().filter(|b| b.idle(now)) {
            backend.connecting = true;
            let (channel, host, address) = (self.clone(), backend.host.clone(), backend.address);
            let attempt = connect(self.tls.clone(), host.clone(), address, backend.authority());
            tokio::spawn(async move {
                let outcome = attempt.await;
                let mut pool = channel.pool.lock().await;
                if let Some(backend) = pool.backends.iter_mut().find(|b| b.address == address && b.host == host)
                    && backend.settle(outcome).is_some()
                {
                    pool.stale = true;
                }
            });
        }
    }

    /// Resolves the hosts again, keeping what it knows of the addresses
    /// still there; a host that does not resolve now keeps its addresses.
    async fn resolve(&self, pool: &mut Pool) -> Result<(), String> {
        let mut known = std::mem::take(&mut pool.backends);
        let mut problem = None;
        for (host, port) in &self.hosts {
            let same = |b: &Backend| b.host == *host && b.port == *port;
            match tokio::net::lookup_host((host.as_str(), *port)).await {
                Ok(addresses) => {
                    for address in addresses {
                        if pool.backends.iter().any(|b| same(b) && b.address == address) {
                            continue;
                        }
                        pool.backends.push(match known.iter().position(|b| same(b) && b.address == address) {
                            Some(at) => known.remove(at),
                            None => Backend {
                                host: host.clone(),
                                port: *port,
                                address,
                                sender: None,
                                connecting: false,
                                retry: None,
                                backoff: FIRST_BACKOFF,
                            },
                        });
                    }
                }
                Err(error) => {
                    problem = Some(format!("cannot resolve {host}: {error}"));
                    let (kept, others) = known.into_iter().partition(|b| same(b));
                    pool.backends.extend::<Vec<Backend>>(kept);
                    known = others;
                }
            }
        }
        if pool.backends.is_empty() {
            pool.stale = true;
            return Err(problem.unwrap_or_else(|| format!("{} has no address", self.target)));
        }
        pool.stale = false;
        Ok(())
    }
}

/// A channel to `target`, `http://host:port` or `https://host:port`, or
/// several `host:port` after the scheme separated by commas, or why there
/// is none; `trust`, `cert` and `key` are its TLS's, as the HTTP client's,
/// and `balancing` 0 for pick first or 1 for round robin.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_channel(
    target: *const Str,
    trust: *const Str,
    cert: *const Str,
    key: *const Str,
    balancing: i64,
) -> *mut NativeObject {
    let target = text(target);
    let made = (|| {
        let (secure, rest) = match (target.strip_prefix("https://"), target.strip_prefix("http://")) {
            (Some(rest), _) => (true, rest),
            (None, Some(rest)) => (false, rest),
            _ => {
                return Err(format!(
                    "`{target}` is no target of gRPC: write `http://host:port`, or `https://host:port` for TLS"
                ));
            }
        };
        let rest = rest.trim_end_matches('/');
        let mut hosts = Vec::new();
        for authority in rest.split(',').map(str::trim) {
            hosts.push(match authority.rsplit_once(':') {
                Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => {
                    let port = port.parse().map_err(|_| format!("`{port}` is not a port"))?;
                    (host.trim_start_matches('[').trim_end_matches(']').to_string(), port)
                }
                _ => (authority.to_string(), if secure { 443 } else { 80 }),
            });
        }
        if hosts.iter().any(|(host, _)| host.is_empty()) {
            return Err(format!("`{target}` names no host: write `http://host:port`"));
        }
        let tls = match secure {
            true => {
                let mut config = crate::http::tls(&text(trust), &text(cert), &text(key), 0)?;
                config.alpn_protocols = vec![b"h2".to_vec()];
                Some(Arc::new(config))
            }
            false => None,
        };
        let balancing = if balancing == 1 { Balancing::RoundRobin } else { Balancing::PickFirst };
        let pool = Pool { backends: Vec::new(), stale: true, given: 0 };
        let target = rest.to_string();
        Ok(Arc::new(Channel { secure, hosts, target, tls, balancing, pool: tokio::sync::Mutex::new(pool) }))
    })();
    native::new(made)
}

/// Why a channel could not be made, or empty.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_channel_error(channel: *mut NativeObject) -> *mut Str {
    match native::get::<Result<Arc<Channel>, String>>(channel) {
        Some(Err(problem)) => str_new(problem.as_bytes()),
        Some(Ok(_)) => str_new(b""),
        None => str_new(b"not a channel"),
    }
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

/// What a call receives, in order.
enum Event {
    Headers(Vec<String>),
    Message(Bytes),
    /// The status: its code, its message and the trailers.
    End(i64, String, Vec<String>),
}

/// Where a call's task puts what arrives, and the status as soon as it
/// comes.
struct Events {
    sender: mpsc::UnboundedSender<Event>,
    ended: Arc<Ended>,
}

impl Events {
    fn send(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    fn end(&self, code: i64, message: String, trailers: Vec<String>) {
        self.ended.set(code, &message, &trailers);
        self.send(Event::End(code, message, trailers));
    }
}

/// A call's status once it ended, while its messages may still wait to be
/// read: what the `next` of interceptors waits for.
#[derive(Default)]
struct Ended {
    status: Mutex<Option<(i64, String, Vec<String>)>>,
    notify: tokio::sync::Notify,
}

impl Ended {
    fn set(&self, code: i64, message: &str, trailers: &[String]) {
        let mut status = lock(&self.status);
        if status.is_none() {
            *status = Some((code, message.to_string(), trailers.to_vec()));
            self.notify.notify_waiters();
        }
    }
}

struct Call {
    /// The messages to send, framed; dropped when the client has sent all.
    outgoing: Mutex<Option<mpsc::Sender<Bytes>>>,
    /// What arrives; a lock of tokio's, which a task that waits on it may
    /// let go of on another thread.
    incoming: tokio::sync::Mutex<mpsc::UnboundedReceiver<Event>>,
    task: tokio::task::JoinHandle<()>,
    headers: Mutex<Vec<String>>,
    /// The message received last, and the status once it came.
    message: Mutex<Bytes>,
    status: Mutex<(i64, String, Vec<String>)>,
    ended: Arc<Ended>,
    /// Whether it sends its messages compressed with gzip.
    compressed: bool,
}

impl Drop for Call {
    fn drop(&mut self) {
        self.task.abort();
        self.ended.set(CANCELLED, "the call was cancelled", &[]);
    }
}

fn pairs(map: &hyper::HeaderMap) -> Vec<String> {
    map.iter()
        .flat_map(|(name, value)| [name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()])
        .collect()
}

/// The status a map of headers or trailers holds, if it holds one.
fn status_of(map: &hyper::HeaderMap) -> Option<(i64, String)> {
    let code = map.get("grpc-status")?.to_str().ok()?.trim().parse().unwrap_or(UNKNOWN);
    let message = map.get("grpc-message").and_then(|m| m.to_str().ok()).map(percent_decoded).unwrap_or_default();
    Some((code, message))
}

/// `grpc-message`, which gRPC percent-encodes.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = text.get(at + 1..at + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The code of gRPC for an HTTP status that is not 200, as gRPC maps them.
fn code_of_http(status: u16) -> i64 {
    match status {
        400 => INTERNAL,
        401 => 16,
        403 => 7,
        404 => 12,
        429 | 502..=504 => UNAVAILABLE,
        _ => UNKNOWN,
    }
}

/// Runs a call: sends its request, then gives what comes back as events.
async fn exchange(channel: Arc<Channel>, mut request: hyper::Request<Body>, events: Events) {
    let end = |code: i64, message: String| events.end(code, message, Vec::new());
    // A connection that closed between calls is let go, and the call
    // takes another.
    let mut tries = 0;
    let (mut sender, authority) = loop {
        let (mut sender, authority) = match channel.pick().await {
            Ok(picked) => picked,
            Err(problem) => return end(UNAVAILABLE, problem),
        };
        tries += 1;
        match sender.ready().await {
            Ok(()) => break (sender, authority),
            Err(error) if tries == 3 => {
                return end(UNAVAILABLE, format!("the connection to {authority} closed: {error}"));
            }
            Err(_) => {}
        }
    };
    let scheme = if channel.secure { "https" } else { "http" };
    match format!("{scheme}://{authority}{}", request.uri().path()).parse() {
        Ok(uri) => *request.uri_mut() = uri,
        Err(error) => return end(INTERNAL, format!("the call's path is not valid: {error}")),
    }
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(error) => return end(UNAVAILABLE, format!("the request to {authority} failed: {error}")),
    };
    let (head, mut body) = response.into_parts();
    if head.status != 200 {
        return end(code_of_http(head.status.as_u16()), format!("the server answered HTTP {}", head.status));
    }
    events.send(Event::Headers(pairs(&head.headers)));
    let encoding = head.headers.get("grpc-encoding").and_then(|v| v.to_str().ok()).unwrap_or("identity").to_string();
    // A status in the headers is the whole answer.
    if let Some((code, message)) = status_of(&head.headers) {
        return events.end(code, message, pairs(&head.headers));
    }
    let mut buffer = BytesMut::new();
    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => return end(UNAVAILABLE, format!("the reply stopped: {error}")),
        };
        match frame.into_data() {
            Ok(data) => {
                buffer.extend_from_slice(&data);
                while buffer.len() >= 5 {
                    let length = u32::from_be_bytes([buffer[1], buffer[2], buffer[3], buffer[4]]) as usize;
                    if buffer.len() < 5 + length {
                        break;
                    }
                    let compressed = buffer[0] != 0;
                    buffer.advance(5);
                    let message = buffer.split_to(length).freeze();
                    let message = match (compressed, encoding.as_str()) {
                        (false, _) => message,
                        (true, "gzip") => match gunzip(&message) {
                            Ok(message) => Bytes::from(message),
                            Err((code, problem)) => return end(code, format!("a message of the server {problem}")),
                        },
                        (true, other) => {
                            let problem = format!(
                                "the server compressed a message with `{other}`, which this client does not read"
                            );
                            return end(INTERNAL, problem);
                        }
                    };
                    events.send(Event::Message(message));
                }
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    let (code, message) =
                        status_of(&trailers).unwrap_or((INTERNAL, "the server gave no status".into()));
                    return events.end(code, message, pairs(&trailers));
                }
            }
        }
    }
    end(INTERNAL, "the reply ended without a status".into())
}

/// A call of `path`, `/package.Service/Method`, with the headers of its
/// metadata as names and values; with `single`, `message` is its one
/// request and it sends no more, and without, its messages follow with
/// `send`. `None` for what is not a channel.
fn begin(
    channel: *mut NativeObject,
    path: *const Str,
    headers: *mut ArrayHeader,
    message: *mut ArrayHeader,
    single: bool,
) -> Option<Call> {
    let channel = match native::get::<Result<Arc<Channel>, String>>(channel)? {
        Ok(channel) => channel.clone(),
        Err(problem) => return Some(finished(UNAVAILABLE, problem.clone())),
    };
    // The address the call goes to gives the rest of its URI.
    let mut request = hyper::Request::post(text(path))
        .header("content-type", "application/grpc+proto")
        .header("te", "trailers")
        .header("grpc-accept-encoding", "gzip")
        .header("user-agent", concat!("grpc-tondo/", env!("CARGO_PKG_VERSION")));
    // A call whose metadata says `grpc-encoding: gzip` compresses its messages.
    let mut compressed = false;
    for pair in crate::http::strings(headers).chunks(2) {
        if let [name, value] = pair {
            compressed |= name == "grpc-encoding" && value == "gzip";
            request = request.header(name.as_str(), value.as_str());
        }
    }
    let (outgoing, mut messages) = mpsc::channel::<Bytes>(16);
    let body = futures_util::stream::poll_fn(move |context| {
        messages.poll_recv(context).map(|m| m.map(|b| Ok(Frame::data(b))))
    });
    let request = match request.body(BodyExt::boxed_unsync(StreamBody::new(body))) {
        Ok(request) => request,
        Err(error) => return Some(finished(INTERNAL, format!("the call's metadata is not valid: {error}"))),
    };
    if single {
        let _ = outgoing.try_send(framed(&unsafe { array_bytes(message) }, compressed));
    }
    let (sender, incoming) = mpsc::unbounded_channel();
    let ended = Arc::new(Ended::default());
    let task = crate::http::runtime().spawn(exchange(channel, request, Events { sender, ended: ended.clone() }));
    Some(Call {
        outgoing: Mutex::new((!single).then_some(outgoing)),
        incoming: tokio::sync::Mutex::new(incoming),
        task,
        headers: Mutex::new(Vec::new()),
        message: Mutex::new(Bytes::new()),
        status: Mutex::new((UNKNOWN, String::new(), Vec::new())),
        ended,
        compressed,
    })
}

/// A call that ended before it began, with this status.
fn finished(code: i64, message: String) -> Call {
    let (sender, incoming) = mpsc::unbounded_channel();
    let ended = Arc::new(Ended::default());
    Events { sender, ended: ended.clone() }.end(code, message, Vec::new());
    Call {
        outgoing: Mutex::new(None),
        incoming: tokio::sync::Mutex::new(incoming),
        task: crate::http::runtime().spawn(async {}),
        headers: Mutex::new(Vec::new()),
        message: Mutex::new(Bytes::new()),
        status: Mutex::new((UNKNOWN, String::new(), Vec::new())),
        ended,
        compressed: false,
    }
}

/// A message as gRPC frames it: whether it is compressed, its length and
/// its bytes.
fn framed(message: &[u8], compressed: bool) -> Bytes {
    let packed;
    let message = if compressed {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = std::io::Write::write_all(&mut encoder, message);
        packed = encoder.finish().unwrap_or_default();
        &packed[..]
    } else {
        message
    };
    let mut framed = BytesMut::with_capacity(5 + message.len());
    framed.extend_from_slice(&[compressed as u8]);
    framed.extend_from_slice(&(message.len() as u32).to_be_bytes());
    framed.extend_from_slice(message);
    framed.freeze()
}

/// The most a compressed message may grow to, so that a small one cannot
/// take all the memory.
const MOST_DECOMPRESSED: u64 = 16 << 20;

/// A message compressed with gzip, or the code and why it is not one.
fn gunzip(message: &[u8]) -> Result<Vec<u8>, (i64, &'static str)> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut decoder = flate2::read::MultiGzDecoder::new(message).take(MOST_DECOMPRESSED + 1);
    if decoder.read_to_end(&mut out).is_err() {
        return Err((INTERNAL, "does not decompress"));
    }
    if out.len() as u64 > MOST_DECOMPRESSED {
        return Err((RESOURCE_EXHAUSTED, "is over 16 MiB once decompressed"));
    }
    Ok(out)
}

/// Starts a call (`begin`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_start(
    channel: *mut NativeObject,
    path: *const Str,
    headers: *mut ArrayHeader,
    message: *mut ArrayHeader,
    single: u8,
) -> *mut NativeObject {
    begin(channel, path, headers, message, single != 0).map_or(std::ptr::null_mut(), native::new)
}

fn call<'a>(object: *mut NativeObject) -> Option<&'a Call> {
    native::get::<Call>(object)
}

/// Sends a message; false once the call ended.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_send(object: *mut NativeObject, message: *mut ArrayHeader) -> u8 {
    let message = unsafe { array_bytes(message) };
    let Some(call) = call(object) else { return 0 };
    let Some(sender) = lock(&call.outgoing).clone() else { return 0 };
    matches!(wait_for(sender.send(framed(&message, call.compressed))), Some(Ok(()))) as u8
}

/// Says the client sent all its messages.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_close_send(object: *mut NativeObject) {
    if let Some(call) = call(object) {
        lock(&call.outgoing).take();
    }
}

/// Waits for what comes next: 1 for a message, which `message` gives, or 0
/// for the end, whose status `code`, `detail` and `trailers` give; -1 when
/// the task is cancelled, which ends the call.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_next(object: *mut NativeObject) -> i64 {
    let Some(call) = call(object) else { return 0 };
    loop {
        let Some(next) = wait_for(async { call.incoming.lock().await.recv().await }) else {
            call.task.abort();
            *lock(&call.status) = (CANCELLED, "the call was cancelled".into(), Vec::new());
            return -1;
        };
        match next {
            Some(Event::Headers(headers)) => *lock(&call.headers) = headers,
            Some(Event::Message(message)) => {
                *lock(&call.message) = message;
                return 1;
            }
            Some(Event::End(code, message, trailers)) => {
                *lock(&call.status) = (code, message, trailers);
                return 0;
            }
            None => return 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_message(object: *mut NativeObject) -> *mut ArrayHeader {
    call(object).map_or_else(|| byte_array(&[]), |c| byte_array(&lock(&c.message)))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_code(object: *mut NativeObject) -> i64 {
    call(object).map_or(UNKNOWN, |c| lock(&c.status).0)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_detail(object: *mut NativeObject) -> *mut Str {
    call(object).map_or_else(|| str_new(b""), |c| str_new(lock(&c.status).1.as_bytes()))
}

/// The headers the server answered with, as names and values.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_headers(object: *mut NativeObject) -> *mut ArrayHeader {
    crate::http::string_array(&call(object).map_or(Vec::new(), |c| lock(&c.headers).clone()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_trailers(object: *mut NativeObject) -> *mut ArrayHeader {
    crate::http::string_array(&call(object).map_or(Vec::new(), |c| lock(&c.status).2.clone()))
}

// ---------------------------------------------------------------------------
// Calls through a client's interceptors
// ---------------------------------------------------------------------------

/// Where the `next` of a client's interceptors puts the call it makes,
/// for the code that called to take; or the status the interceptors ended
/// with when they made none.
#[derive(Default)]
struct Slot {
    held: Mutex<Held>,
    notify: tokio::sync::Notify,
}

#[derive(Default)]
enum Held {
    #[default]
    Waiting,
    /// Boxed, as a call is large where a lock is, as on macOS.
    Made(Box<Call>),
    Taken,
    Closed(i64, String),
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_slot() -> *mut NativeObject {
    native::new(Slot::default())
}

/// `next`: makes the call, which the slot keeps for the caller in place of
/// one made before, and gives what says when it ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_open(
    slot: *mut NativeObject,
    channel: *mut NativeObject,
    path: *const Str,
    headers: *mut ArrayHeader,
    message: *mut ArrayHeader,
    single: u8,
) -> *mut NativeObject {
    let (Some(slot), Some(call)) = (native::get::<Slot>(slot), begin(channel, path, headers, message, single != 0))
    else {
        return std::ptr::null_mut();
    };
    let ended = call.ended.clone();
    // A call made after the caller took one is nobody's, and ends.
    let _replaced = {
        let mut held = lock(&slot.held);
        match *held {
            Held::Taken => Some(Box::new(call)),
            _ => match std::mem::replace(&mut *held, Held::Made(Box::new(call))) {
                Held::Made(before) => Some(before),
                _ => None,
            },
        }
    };
    slot.notify.notify_waiters();
    native::new(ended)
}

/// Waits until a call ends and gives its status: its code, its message and
/// the trailers' names and values.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_ended(object: *mut NativeObject) -> *mut ArrayHeader {
    let Some(ended) = native::get::<Arc<Ended>>(object).cloned() else {
        return crate::http::string_array(&[]);
    };
    let status = wait_for(async move {
        loop {
            let notified = ended.notify.notified();
            if let Some(status) = lock(&ended.status).clone() {
                return status;
            }
            notified.await;
        }
    });
    let (code, message, trailers) = status.unwrap_or((CANCELLED, "the call was cancelled".into(), Vec::new()));
    let mut list = vec![code.to_string(), message];
    list.extend(trailers);
    crate::http::string_array(&list)
}

/// The call the interceptors made, once `next` made it; or one that ended
/// with the status they closed the slot with.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_take(object: *mut NativeObject) -> *mut NativeObject {
    let Some(slot) = native::get::<Slot>(object) else { return std::ptr::null_mut() };
    let taken = wait_for(async {
        loop {
            let notified = slot.notify.notified();
            {
                let mut held = lock(&slot.held);
                match std::mem::replace(&mut *held, Held::Taken) {
                    Held::Made(call) => return *call,
                    Held::Closed(code, message) => return finished(code, message),
                    other => *held = other,
                }
            }
            notified.await;
        }
    });
    native::new(taken.unwrap_or_else(|| finished(CANCELLED, "the call was cancelled".into())))
}

/// The interceptors returned: if they made no call, the caller's ends with
/// `code` and `message`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_close(object: *mut NativeObject, code: i64, message: *const Str) {
    if let Some(slot) = native::get::<Slot>(object) {
        let mut held = lock(&slot.held);
        if matches!(*held, Held::Waiting) {
            *held = Held::Closed(code, text(message));
        }
        drop(held);
        slot.notify.notify_waiters();
    }
}

// ---------------------------------------------------------------------------
// The messages a server reads
// ---------------------------------------------------------------------------

/// The frames of a request's body: a stream of `std.io`, its bytes read
/// and not yet a whole frame, and the message read last or why reading
/// failed.
struct Frames {
    /// `None` for frames given whole.
    source: Option<crate::http::Source>,
    buffer: BytesMut,
    message: Bytes,
    error: String,
    /// The request's `grpc-encoding`.
    encoding: String,
    /// The Base64 text of a gRPC-Web body sent as text not yet decoded, or
    /// `None` for a body of bytes.
    text: Option<Vec<u8>>,
}

/// The frames of a request body's stream, compressed with `encoding`, and
/// in Base64 text with `text`, as `application/grpc-web-text` sends them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_frames(
    input: *mut crate::io::StreamObject,
    encoding: *const Str,
    text_body: u8,
) -> *mut NativeObject {
    unsafe { crate::tasks::tondo_rt_handle_retain(input as *mut u8) };
    let frames = Frames {
        source: Some(crate::http::Source(input)),
        buffer: BytesMut::new(),
        message: Bytes::new(),
        error: String::new(),
        encoding: text(encoding),
        text: (text_body != 0).then(Vec::new),
    };
    native::new(Mutex::new(frames))
}

/// Frames given whole, compressed with `encoding`, as the message of a
/// unary call of Connect.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_frames_of(bytes: *mut ArrayHeader, encoding: *const Str) -> *mut NativeObject {
    let frames = Frames {
        source: None,
        buffer: BytesMut::from(&unsafe { array_bytes(bytes) }[..]),
        message: Bytes::new(),
        error: String::new(),
        encoding: text(encoding),
        text: None,
    };
    native::new(Mutex::new(frames))
}

/// The bytes of the whole groups of four characters of Base64 text, which
/// leaves the rest for the next piece. A client may pad each piece it
/// sends, so a group that ends in `=` ends a piece.
fn decoded(text: &mut Vec<u8>) -> Result<Vec<u8>, usize> {
    let whole = text.len() / 4 * 4;
    let mut bytes = Vec::with_capacity(whole / 4 * 3);
    let mut start = 0;
    for group in (0..whole).step_by(4) {
        if text[group + 3] == b'=' || group + 4 == whole {
            bytes.extend(crate::codec::base64_decode(&text[start..group + 4], false).map_err(|at| start + at)?);
            start = group + 4;
        }
    }
    text.drain(..whole);
    Ok(bytes)
}

/// Reads the next message: 1 when there is one, which `frame` gives, 0 at
/// the end of the body, -1 when it is not gRPC's, -2 when it is compressed
/// as this server does not read, or -3 when it decompresses to too much,
/// which `frameError` says.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_read_frame(object: *mut NativeObject) -> i64 {
    let Some(frames) = native::get::<Mutex<Frames>>(object) else { return -1 };
    let mut frames = lock(frames);
    loop {
        if frames.buffer.len() >= 5 {
            let length =
                u32::from_be_bytes([frames.buffer[1], frames.buffer[2], frames.buffer[3], frames.buffer[4]]) as usize;
            if frames.buffer.len() >= 5 + length {
                let compressed = frames.buffer[0] != 0;
                frames.buffer.advance(5);
                let message = frames.buffer.split_to(length).freeze();
                if !compressed {
                    frames.message = message;
                    return 1;
                }
                if frames.encoding != "gzip" {
                    frames.error = format!(
                        "the client compressed a message with `{}`, which this server does not read: send `grpc-encoding: gzip`",
                        frames.encoding
                    );
                    return -2;
                }
                match gunzip(&message) {
                    Ok(message) => {
                        frames.message = Bytes::from(message);
                        return 1;
                    }
                    Err((code, problem)) => {
                        frames.error = format!("a message of the client {problem}");
                        return if code == RESOURCE_EXHAUSTED { -3 } else { -1 };
                    }
                }
            }
        }
        let read = match &frames.source {
            Some(source) => crate::io::run(source.0, |s| s.read(64 * 1024)),
            None => Ok(None),
        };
        match read {
            Ok(Some(chunk)) => {
                let Some(text) = frames.text.as_mut() else {
                    frames.buffer.extend_from_slice(&chunk);
                    continue;
                };
                text.extend_from_slice(&chunk);
                match decoded(text) {
                    Ok(bytes) => frames.buffer.extend_from_slice(&bytes),
                    Err(at) => {
                        frames.error = format!("the request's Base64 text is wrong near character {at} of its piece");
                        return -1;
                    }
                }
            }
            Ok(None) if frames.text.as_ref().is_some_and(|text| !text.is_empty()) => {
                frames.error = "the request's Base64 text ends in the middle of a group of four characters".into();
                return -1;
            }
            Ok(None) if frames.buffer.is_empty() => return 0,
            Ok(None) => {
                frames.error = "the request ended inside a message".into();
                return -1;
            }
            Err(error) => {
                frames.error = format!("cannot read the request: {error}");
                return -1;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_frame(object: *mut NativeObject) -> *mut ArrayHeader {
    native::get::<Mutex<Frames>>(object).map_or_else(|| byte_array(&[]), |f| byte_array(&lock(f).message))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_frame_error(object: *mut NativeObject) -> *mut Str {
    native::get::<Mutex<Frames>>(object).map_or_else(|| str_new(b""), |f| str_new(lock(f).error.as_bytes()))
}

// ---------------------------------------------------------------------------
// A client that goes
// ---------------------------------------------------------------------------

/// Cancels the running task if the reader of the pipe `writer` writes to is
/// gone before the writer: the server dropped the reply, as its client
/// went, or dropped the request's handler before it answered.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_follow(writer: *mut crate::io::StreamObject) {
    let Some(gone) = crate::io::other_end_gone(writer) else { return };
    let task = crate::tasks::current_id();
    crate::http::runtime().spawn(async move {
        if gone.await {
            crate::tasks::cancel_by_id(task);
        }
    });
}

// ---------------------------------------------------------------------------
// When a server's headers may go
// ---------------------------------------------------------------------------

/// Set once, by the method's first message or its end; waited for by the
/// task that answers with the headers.
#[derive(Default)]
struct Signal {
    set: std::sync::atomic::AtomicBool,
    notify: tokio::sync::Notify,
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_signal() -> *mut NativeObject {
    native::new(Arc::new(Signal::default()))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_signal_set(object: *mut NativeObject) {
    if let Some(signal) = native::get::<Arc<Signal>>(object) {
        signal.set.store(true, std::sync::atomic::Ordering::SeqCst);
        signal.notify.notify_waiters();
    }
}

/// Waits until the signal is set; false when the task is cancelled first.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_grpc_signal_wait(object: *mut NativeObject) -> u8 {
    let Some(signal) = native::get::<Arc<Signal>>(object).cloned() else { return 0 };
    let waited = wait_for(async move {
        loop {
            let notified = signal.notify.notified();
            if signal.set.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    });
    waited.is_some() as u8
}
