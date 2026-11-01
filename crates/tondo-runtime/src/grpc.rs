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
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

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

/// Where a channel connects, and its connection once it has one.
struct Channel {
    /// `http` or `https`, the host, the port and `host:port`.
    secure: bool,
    host: String,
    port: u16,
    authority: String,
    tls: Option<Arc<rustls::ClientConfig>>,
    sender: tokio::sync::Mutex<Option<SendRequest<Body>>>,
}

impl Channel {
    /// The connection, made again when there is none or it closed.
    async fn connection(&self) -> Result<SendRequest<Body>, String> {
        let mut slot = self.sender.lock().await;
        if let Some(sender) = slot.as_ref().filter(|s| !s.is_closed()) {
            return Ok(sender.clone());
        }
        let tcp = tokio::net::TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|error| format!("cannot connect to {}: {error}", self.authority))?;
        let _ = tcp.set_nodelay(true);
        let sender = match &self.tls {
            Some(config) => {
                let name = rustls_pki_types::ServerName::try_from(self.host.clone())
                    .map_err(|_| format!("`{}` is not a name TLS checks", self.host))?;
                let stream = tokio_rustls::TlsConnector::from(config.clone())
                    .connect(name, tcp)
                    .await
                    .map_err(|error| format!("TLS with {} failed: {error}", self.authority))?;
                let (sender, connection) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(stream))
                        .await
                        .map_err(|error| format!("HTTP/2 with {} failed: {error}", self.authority))?;
                tokio::spawn(connection);
                sender
            }
            None => {
                let (sender, connection) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
                        .await
                        .map_err(|error| format!("HTTP/2 with {} failed: {error}", self.authority))?;
                tokio::spawn(connection);
                sender
            }
        };
        *slot = Some(sender.clone());
        Ok(sender)
    }
}

/// A channel to `target`, `http://host:port` or `https://host:port`, or
/// why there is none; `trust`, `cert` and `key` are its TLS's, as the HTTP
/// client's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_channel(
    target: *const Str,
    trust: *const Str,
    cert: *const Str,
    key: *const Str,
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
        let authority = rest.trim_end_matches('/').to_string();
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !host.ends_with(']') || host.starts_with('[') => {
                let port = port.parse().map_err(|_| format!("`{port}` is not a port"))?;
                (host.trim_start_matches('[').trim_end_matches(']').to_string(), port)
            }
            _ => (authority.clone(), if secure { 443 } else { 80 }),
        };
        let tls = match secure {
            true => {
                let mut config = crate::http::tls(&text(trust), &text(cert), &text(key), 0)?;
                config.alpn_protocols = vec![b"h2".to_vec()];
                Some(Arc::new(config))
            }
            false => None,
        };
        Ok(Arc::new(Channel { secure, host, port, authority, tls, sender: tokio::sync::Mutex::new(None) }))
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
async fn exchange(channel: Arc<Channel>, request: hyper::Request<Body>, events: Events) {
    let end = |code: i64, message: String| events.end(code, message, Vec::new());
    let mut sender = match channel.connection().await {
        Ok(sender) => sender,
        Err(problem) => return end(UNAVAILABLE, problem),
    };
    if sender.ready().await.is_err() {
        // The connection closed between calls: one more try on a new one.
        *channel.sender.lock().await = None;
        sender = match channel.connection().await {
            Ok(sender) => sender,
            Err(problem) => return end(UNAVAILABLE, problem),
        };
    }
    let response = match sender.send_request(request).await {
        Ok(response) => response,
        Err(error) => return end(UNAVAILABLE, format!("the request to {} failed: {error}", channel.authority)),
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
    let scheme = if channel.secure { "https" } else { "http" };
    let mut request = hyper::Request::post(format!("{scheme}://{}{}", channel.authority, text(path)))
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
    source: crate::http::Source,
    buffer: BytesMut,
    message: Bytes,
    error: String,
    /// The request's `grpc-encoding`.
    encoding: String,
}

/// The frames of a request body's stream, compressed with `encoding`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_grpc_frames(
    input: *mut crate::io::StreamObject,
    encoding: *const Str,
) -> *mut NativeObject {
    unsafe { crate::tasks::tondo_rt_handle_retain(input as *mut u8) };
    let frames = Frames {
        source: crate::http::Source(input),
        buffer: BytesMut::new(),
        message: Bytes::new(),
        error: String::new(),
        encoding: text(encoding),
    };
    native::new(Mutex::new(frames))
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
        let read = crate::io::run(frames.source.0, |s| s.read(64 * 1024));
        match read {
            Ok(Some(chunk)) => frames.buffer.extend_from_slice(&chunk),
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
