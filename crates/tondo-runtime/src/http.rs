//! What `std.http` does in the runtime (standard library 14.24): requests
//! over HTTP/1.1 and HTTP/2, with TLS, on a tokio runtime of its own. A task
//! waits for a request without holding a worker; a response's body reads
//! as a stream of `std.io`.

use super::native::{self, NativeObject};
use super::tasks::{CANCELLED, HOST_WORK, Wait, current, notify};
use super::{ArrayHeader, Str, element, str_bytes, str_new, tondo_rt_array_new};
use crate::io::{Host, Mode, StreamObject, byte_array};
use bytes::{Buf, Bytes};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        // A write to a connection the other side closed, which `writev`
        // makes without `MSG_NOSIGNAL`, fails instead of ending the program,
        // as in Rust programs; console programs keep the default.
        unsafe extern "C" {
            fn signal(signal: i32, handler: usize) -> usize;
        }
        const SIGPIPE: i32 = 13;
        const SIG_IGN: usize = 1;
        unsafe { signal(SIGPIPE, SIG_IGN) };
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("tondo-http")
            .enable_all()
            .build()
            .expect("the HTTP runtime starts")
    })
}

/// What a future gives when the current task waits for it on the HTTP
/// runtime; `None` when the task is cancelled, which drops the future.
pub(crate) fn wait<T: Send + 'static>(future: impl Future<Output = T> + Send + 'static) -> Option<T> {
    if current().is_null() {
        return Some(runtime().block_on(future));
    }
    // Wakes the task when the future ends or is dropped.
    struct Done {
        wait: Arc<Wait>,
        ended: Arc<(Mutex<bool>, Condvar)>,
        virtual_clock: bool,
    }
    impl Drop for Done {
        fn drop(&mut self) {
            *self.ended.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            self.ended.1.notify_all();
            if self.wait.claim(0) {
                notify(self.wait.task);
            }
            if self.virtual_clock {
                super::clock::external(false);
            }
            HOST_WORK.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let slot = Arc::new(Mutex::new(None));
    let (waiting, ended) = (Wait::new(), Arc::new((Mutex::new(false), Condvar::new())));
    HOST_WORK.fetch_add(1, Ordering::SeqCst);
    // A virtual clock does not move while its tasks wait on the host.
    let virtual_clock = super::clock::in_domain();
    super::clock::external(true);
    let done = Done { wait: waiting.clone(), ended: ended.clone(), virtual_clock };
    let out = slot.clone();
    let handle = runtime().spawn(async move {
        let _done = done;
        let value = future.await;
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
    });
    let cancelled = waiting.block() == CANCELLED;
    if cancelled {
        handle.abort();
    }
    let mut over = ended.0.lock().unwrap_or_else(|e| e.into_inner());
    while !*over {
        over = ended.1.wait(over).unwrap_or_else(|e| e.into_inner());
    }
    drop(over);
    if cancelled {
        return None;
    }
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub(crate) fn text(s: *const Str) -> String {
    String::from_utf8_lossy(unsafe { str_bytes(s) }).into_owned()
}

pub(crate) fn strings(array: *mut ArrayHeader) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let count = unsafe { (*array).len };
    (0..count).map(|i| text(unsafe { *(element(array, 8, i) as *const *const Str) })).collect()
}

pub(crate) fn string_array(items: &[String]) -> *mut ArrayHeader {
    let array = tondo_rt_array_new(8, items.len());
    for (index, item) in items.iter().enumerate() {
        unsafe { *(element(array, 8, index) as *mut *mut Str) = str_new(item.as_bytes()) };
    }
    array
}

pub(crate) fn bytes_of(array: *mut ArrayHeader) -> Vec<u8> {
    if array.is_null() {
        return Vec::new();
    }
    unsafe { (0..(*array).len).map(|i| *element(array, 1, i)).collect() }
}

/// The roots TLS trusts: the system's, or Mozilla's when the system has
/// none, as in a minimal container.
fn roots() -> rustls::RootCertStore {
    static ROOTS: OnceLock<rustls::RootCertStore> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            for cert in rustls_native_certs::load_native_certs().certs {
                let _ = roots.add(cert);
            }
            if roots.is_empty() {
                roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            roots
        })
        .clone()
}

/// A client's TLS: the roots, and the certificates of a PEM file when one is
/// named; ALPN offers HTTP/2 first.
pub(crate) fn tls(trust: &str) -> Result<rustls::ClientConfig, String> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    let mut roots = roots();
    if !trust.is_empty() {
        let certs = CertificateDer::pem_file_iter(trust)
            .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
            .map_err(|error| format!("the trusted certificates `{trust}`: {error}"))?;
        for cert in certs {
            roots.add(cert).map_err(|error| format!("the trusted certificates `{trust}`: {error}"))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// A client: its connections, reused by its requests, or why it has none.
/// `redirects` is how many it follows; a connection waits `connect_ms`, or
/// forever with 0; `trust` names a PEM file of more certificates to trust.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_client(connect_ms: i64, redirects: i64, trust: *const Str) -> *mut NativeObject {
    let config = match tls(&text(trust)) {
        Ok(config) => config,
        Err(problem) => return native::new(Err::<reqwest::Client, String>(problem)),
    };
    let mut builder = reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .user_agent(concat!("tondo/", env!("CARGO_PKG_VERSION")))
        .redirect(match redirects {
            0 => reqwest::redirect::Policy::none(),
            n => reqwest::redirect::Policy::limited(n.max(0) as usize),
        });
    if connect_ms > 0 {
        builder = builder.connect_timeout(Duration::from_millis(connect_ms as u64));
    }
    native::new(builder.build().map_err(|error| error.to_string()))
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// A stream of `std.io` that a body is read from; it holds a reference.
pub(crate) struct Source(pub(crate) *mut StreamObject);

unsafe impl Send for Source {}
unsafe impl Sync for Source {}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe { crate::io::tondo_rt_stream_release(self.0) }
    }
}

enum Body {
    Empty,
    Bytes(Bytes, String),
    Stream(Mutex<Option<Source>>, i64, String),
    Form(Mutex<Option<Form>>),
}

/// A multipart form under construction, or the first problem of a part.
type Form = Result<reqwest::multipart::Form, String>;

/// The body's chunks, read from the stream on blocking threads.
fn chunks(source: Source) -> reqwest::Body {
    reqwest::Body::wrap_stream(stream_of(source))
}

/// The chunks a stream of `std.io` gives, read on blocking threads.
pub(crate) fn stream_of(
    source: Source,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(Arc::new(source), |source| async move {
        let reading = source.clone();
        let read = tokio::task::spawn_blocking(move || crate::io::run(reading.0, |s| s.read(64 * 1024))).await;
        match read {
            Ok(Ok(Some(bytes))) => Some((Ok(Bytes::from(bytes)), source)),
            Ok(Ok(None)) => None,
            Ok(Err(error)) => Some((Err(error), source)),
            Err(error) => Some((Err(std::io::Error::other(error)), source)),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_body_empty() -> *mut NativeObject {
    native::new(Body::Empty)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_body_bytes(
    data: *mut ArrayHeader,
    content_type: *const Str,
) -> *mut NativeObject {
    native::new(Body::Bytes(Bytes::from(bytes_of(data)), text(content_type)))
}

/// A body read from a stream as it is sent; `length` is its size when
/// known, or negative.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_body_stream(
    input: *mut StreamObject,
    length: i64,
    content_type: *const Str,
) -> *mut NativeObject {
    unsafe { super::tasks::tondo_rt_handle_retain(input as *mut u8) };
    native::new(Body::Stream(Mutex::new(Some(Source(input))), length, text(content_type)))
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_form() -> *mut NativeObject {
    native::new(Mutex::new(Some(Ok::<_, String>(reqwest::multipart::Form::new()))))
}

/// Adds a part to a form under construction, with its file name and
/// content type when given; a content type that does not parse is the
/// form's problem.
fn add_part(
    form: *mut NativeObject,
    name: String,
    part: reqwest::multipart::Part,
    file_name: String,
    content_type: String,
) {
    let Some(slot) = native::get::<Mutex<Option<Form>>>(form) else { return };
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    let part = if file_name.is_empty() { part } else { part.file_name(file_name) };
    let part = match content_type.is_empty() {
        true => Ok(part),
        false => part.mime_str(&content_type).map_err(|_| format!("`{content_type}` is not a content type")),
    };
    *slot = match (slot.take(), part) {
        (Some(Ok(current)), Ok(part)) => Some(Ok(current.part(name, part))),
        (Some(Ok(_)), Err(problem)) => Some(Err(problem)),
        (other, _) => other,
    };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_text(form: *mut NativeObject, name: *const Str, value: *const Str) {
    add_part(form, text(name), reqwest::multipart::Part::text(text(value)), String::new(), String::new());
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_bytes(
    form: *mut NativeObject,
    name: *const Str,
    data: *mut ArrayHeader,
    file_name: *const Str,
    content_type: *const Str,
) {
    let part = reqwest::multipart::Part::bytes(bytes_of(data));
    add_part(form, text(name), part, text(file_name), text(content_type));
}

/// Adds a file's part, read from disk as it is sent; the error when the
/// file cannot be opened, or an empty string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_file(
    form: *mut NativeObject,
    name: *const Str,
    path: *const Str,
    content_type: *const Str,
) -> *mut Str {
    let path = text(path);
    let opened = wait(async move { reqwest::multipart::Part::file(path).await });
    match opened {
        Some(Ok(part)) => {
            add_part(form, text(name), part, String::new(), text(content_type));
            str_new(b"")
        }
        Some(Err(error)) => str_new(error.to_string().as_bytes()),
        None => str_new(b"the task was cancelled"),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_stream(
    form: *mut NativeObject,
    name: *const Str,
    input: *mut StreamObject,
    file_name: *const Str,
    content_type: *const Str,
    length: i64,
) {
    unsafe { super::tasks::tondo_rt_handle_retain(input as *mut u8) };
    let body = chunks(Source(input));
    let part = match u64::try_from(length) {
        Ok(length) => reqwest::multipart::Part::stream_with_length(body, length),
        Err(_) => reqwest::multipart::Part::stream(body),
    };
    add_part(form, text(name), part, text(file_name), text(content_type));
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_body_form(form: *mut NativeObject) -> *mut NativeObject {
    let taken =
        native::get::<Mutex<Option<Form>>>(form).and_then(|slot| slot.lock().unwrap_or_else(|e| e.into_inner()).take());
    native::new(Body::Form(Mutex::new(taken)))
}

// ---------------------------------------------------------------------------
// Requests and responses
// ---------------------------------------------------------------------------

/// A response, or why there is none: its head, and its body until a
/// reader takes it.
struct Response {
    failure: Failure,
    status: u16,
    reason: String,
    url: String,
    version: String,
    headers: Vec<String>,
    body: Mutex<Option<reqwest::Response>>,
}

impl Response {
    fn failed(kind: &'static str, message: String) -> Response {
        Response {
            failure: Mutex::new(Some((kind, message))),
            status: 0,
            reason: String::new(),
            url: String::new(),
            version: String::new(),
            headers: Vec::new(),
            body: Mutex::new(None),
        }
    }
}

/// What kind of failure a reqwest error is, and its message with its causes.
fn classify(error: &reqwest::Error) -> (&'static str, String) {
    let kind = if error.is_builder() {
        "url"
    } else if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_body() || error.is_decode() {
        "body"
    } else {
        "protocol"
    };
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    (kind, message)
}

/// Sends a request and waits for the head of its response. `headers` are
/// names and values in turn; `timeout_ms` bounds the whole exchange, or
/// nothing with 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_send(
    client: *mut NativeObject,
    method: *const Str,
    url: *const Str,
    headers: *mut ArrayHeader,
    body: *mut NativeObject,
    timeout_ms: i64,
) -> *mut NativeObject {
    super::diagnostics::HTTP_CLIENT_REQUESTS.add(1);
    let client = match native::get::<Result<reqwest::Client, String>>(client) {
        Some(Ok(client)) => client.clone(),
        Some(Err(problem)) => return native::new(Response::failed("connect", problem.clone())),
        None => return native::new(Response::failed("protocol", "not an HTTP client".into())),
    };
    let Ok(method) = reqwest::Method::from_bytes(text(method).as_bytes()) else {
        return native::new(Response::failed("protocol", format!("`{}` is not an HTTP method", text(method))));
    };
    let mut request = client.request(method, text(url));
    let headers = strings(headers);
    for pair in headers.chunks(2) {
        if let [name, value] = pair {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    if timeout_ms > 0 {
        request = request.timeout(Duration::from_millis(timeout_ms as u64));
    }
    let has = |name: &str| headers.chunks(2).any(|pair| pair[0].eq_ignore_ascii_case(name));
    request = match native::get::<Body>(body) {
        None | Some(Body::Empty) => request,
        Some(Body::Bytes(data, content_type)) => {
            let request = if has("content-type") { request } else { request.header("content-type", content_type) };
            request.body(data.clone())
        }
        Some(Body::Stream(source, length, content_type)) => {
            let Some(source) = source.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                return native::new(Response::failed("body", "a stream body is sent once".into()));
            };
            let mut request = if has("content-type") { request } else { request.header("content-type", content_type) };
            if *length >= 0 && !has("content-length") {
                request = request.header("content-length", length.to_string());
            }
            request.body(chunks(source))
        }
        Some(Body::Form(form)) => match form.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(form)) => request.multipart(form),
            Some(Err(problem)) => return native::new(Response::failed("body", problem)),
            None => return native::new(Response::failed("body", "a multipart body is sent once".into())),
        },
    };
    let response = match wait(async move { request.send().await }) {
        None => Response::failed("protocol", "the task was cancelled".into()),
        Some(Err(error)) => {
            let (kind, message) = classify(&error);
            Response::failed(kind, message)
        }
        Some(Ok(response)) => {
            let status = response.status();
            let headers = response
                .headers()
                .iter()
                .flat_map(|(name, value)| [name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()])
                .collect();
            Response {
                failure: Mutex::new(None),
                status: status.as_u16(),
                reason: status.canonical_reason().unwrap_or_default().to_string(),
                url: response.url().to_string(),
                version: format!("{:?}", response.version()),
                headers,
                body: Mutex::new(Some(response)),
            }
        }
    };
    native::new(response)
}

fn response<'a>(object: *mut NativeObject) -> &'a Response {
    native::get::<Response>(object).expect("an HTTP response")
}

/// Why a request or a read of its body failed, as `kind\tmessage`, or an
/// empty string.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_failure(object: *mut NativeObject) -> *mut Str {
    let Some(slot) = failure_of(object) else { return str_new(b"") };
    let failure = slot.lock().unwrap_or_else(|e| e.into_inner());
    match failure.as_ref() {
        Some((kind, message)) => str_new(format!("{kind}\t{message}").as_bytes()),
        None => str_new(b""),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_status(object: *mut NativeObject) -> i64 {
    i64::from(response(object).status)
}

/// The status's reason phrase, its final URL after redirects and the
/// protocol version, as `reason\turl\tversion`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_head(object: *mut NativeObject) -> *mut Str {
    let response = response(object);
    str_new(format!("{}\t{}\t{}", response.reason, response.url, response.version).as_bytes())
}

/// The headers' names, lowercase, and values in turn.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_headers(object: *mut NativeObject) -> *mut ArrayHeader {
    string_array(&response(object).headers)
}

/// Why a second read of a response's body has nothing.
const ALREADY_READ: &str = "it was already read, and a response's body is read once: keep what the first \
    read gave, as `let text = response.text()?`, and read that again, as `text.lines()`";

/// The whole body; on a failure, what was read and the failure recorded.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_read_all(object: *mut NativeObject) -> *mut ArrayHeader {
    let response = response(object);
    let Some(body) = response.body.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        *response.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(("body", ALREADY_READ.into()));
        return byte_array(&[]);
    };
    match wait(async move { body.bytes().await }) {
        Some(Ok(bytes)) => byte_array(&bytes),
        outcome => {
            let failure = match outcome {
                Some(Err(error)) => classify(&error),
                _ => ("protocol", "the task was cancelled".into()),
            };
            *response.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(failure);
            byte_array(&[])
        }
    }
}

/// A response's body as the host of a stream: chunks as they arrive.
struct Chunks {
    body: Option<reqwest::Response>,
    chunk: Bytes,
    /// Whether another read took the body before this one.
    taken: bool,
    /// Whether to give the body's Windows-1252 as UTF-8.
    windows_1252: bool,
}

impl Host for Chunks {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.taken {
            return Err(std::io::Error::other(ALREADY_READ));
        }
        while self.chunk.is_empty() {
            let Some(body) = self.body.as_mut() else { return Ok(0) };
            match runtime().block_on(body.chunk()) {
                Ok(Some(chunk)) if self.windows_1252 => self.chunk = windows_1252(&chunk).into_bytes().into(),
                Ok(Some(chunk)) => self.chunk = chunk,
                Ok(None) => self.body = None,
                Err(error) => return Err(std::io::Error::other(error)),
            }
        }
        let count = buffer.len().min(self.chunk.len());
        buffer[..count].copy_from_slice(&self.chunk[..count]);
        self.chunk.advance(count);
        Ok(count)
    }
}

/// The body as a stream of `std.io`, read as the program asks, in UTF-8
/// when it is Windows-1252; one that fails when another read already took
/// it.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_body(object: *mut NativeObject, windows_1252: u8) -> *mut StreamObject {
    let response = response(object);
    let body = response.body.lock().unwrap_or_else(|e| e.into_inner()).take();
    let taken = body.is_none();
    if taken {
        *response.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(("body", ALREADY_READ.into()));
    }
    crate::io::stream(
        Box::new(Chunks { body, chunk: Bytes::new(), taken, windows_1252: windows_1252 != 0 }),
        Mode::Long,
        false,
    )
}

/// Windows-1252 as text, as browsers read the labels of Latin-1 and ASCII
/// too: Latin-1 but for its own characters from 0x80 to 0x9F.
fn windows_1252(bytes: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}', '\u{90}', '‘',
        '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    ];
    bytes.iter().map(|&b| if (0x80..0xa0).contains(&b) { HIGH[usize::from(b - 0x80)] } else { char::from(b) }).collect()
}

/// Bytes of Windows-1252 as text.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_windows_1252(data: *mut ArrayHeader) -> *mut Str {
    str_new(windows_1252(&bytes_of(data)).as_bytes())
}

// ---------------------------------------------------------------------------
// WebSockets
// ---------------------------------------------------------------------------

type Frame = tokio_tungstenite::tungstenite::Message;
type FrameSink =
    std::pin::Pin<Box<dyn futures_util::Sink<Frame, Error = tokio_tungstenite::tungstenite::Error> + Send>>;
type FrameStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<Frame, tokio_tungstenite::tungstenite::Error>> + Send>>;
pub(crate) type Failure = Mutex<Option<(&'static str, String)>>;

/// A WebSocket's halves, each behind its own lock: one task can send while
/// another receives.
pub(crate) struct Socket {
    pub(crate) failure: Failure,
    sink: Option<Arc<tokio::sync::Mutex<FrameSink>>>,
    stream: Option<Arc<tokio::sync::Mutex<FrameStream>>>,
}

impl Socket {
    pub(crate) fn failed(kind: &'static str, message: String) -> Socket {
        Socket { failure: Mutex::new(Some((kind, message))), sink: None, stream: None }
    }
}

/// The socket of a connection a client opened or a server accepted.
pub(crate) fn socket<S>(connection: tokio_tungstenite::WebSocketStream<S>) -> Socket
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use futures_util::StreamExt;
    let (sink, stream) = connection.split();
    Socket {
        failure: Mutex::new(None),
        sink: Some(Arc::new(tokio::sync::Mutex::new(Box::pin(sink)))),
        stream: Some(Arc::new(tokio::sync::Mutex::new(Box::pin(stream)))),
    }
}

/// A message received: text (0), binary (1) or the end (2), or a failure.
struct Received {
    failure: Failure,
    kind: i64,
    text: String,
    data: Vec<u8>,
}

/// Where an object of this module keeps its failure.
fn failure_of<'a>(object: *mut NativeObject) -> Option<&'a Failure> {
    native::get::<Response>(object)
        .map(|r| &r.failure)
        .or_else(|| native::get::<Socket>(object).map(|s| &s.failure))
        .or_else(|| native::get::<Received>(object).map(|r| &r.failure))
        .or_else(|| super::server::failure_of(object))
}

fn ws_failure(error: tokio_tungstenite::tungstenite::Error) -> (&'static str, String) {
    use tokio_tungstenite::tungstenite::Error;
    let kind = match &error {
        Error::Url(_) => "url",
        Error::Io(_) | Error::Tls(_) => "connect",
        _ => "protocol",
    };
    (kind, error.to_string())
}

/// Opens a WebSocket (RFC 6455) with the headers, names and values in
/// turn; the handshake waits `timeout_ms`, or forever with 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_ws_connect(
    url: *const Str,
    headers: *mut ArrayHeader,
    timeout_ms: i64,
) -> *mut NativeObject {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
    let failed = |kind: &'static str, message: String| native::new(Socket::failed(kind, message));
    let mut request = match text(url).into_client_request() {
        Ok(request) => request,
        Err(error) => return failed("url", error.to_string()),
    };
    for pair in strings(headers).chunks(2) {
        if let [name, value] = pair {
            match (HeaderName::try_from(name.as_str()), HeaderValue::try_from(value.as_str())) {
                (Ok(name), Ok(value)) => {
                    request.headers_mut().insert(name, value);
                }
                _ => return failed("protocol", format!("`{name}: {value}` is not a header")),
            }
        }
    }
    // The WebSocket handshake is HTTP/1.1.
    let mut config = match tls("") {
        Ok(config) => config,
        Err(problem) => return failed("connect", problem),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_tungstenite::Connector::Rustls(Arc::new(config));
    let connecting = tokio_tungstenite::connect_async_tls_with_config(request, None, false, Some(connector));
    let outcome = wait(async move {
        match timeout_ms {
            0 => connecting.await.map_err(ws_failure),
            limit => match tokio::time::timeout(Duration::from_millis(limit.max(0) as u64), connecting).await {
                Ok(connected) => connected.map_err(ws_failure),
                Err(_) => Err(("timeout", "the handshake timed out".into())),
            },
        }
    });
    match outcome {
        None => failed("protocol", "the task was cancelled".into()),
        Some(Err((kind, message))) => failed(kind, message),
        Some(Ok((connection, _))) => native::new(socket(connection)),
    }
}

/// Sends a text or, with `binary`, a binary message; the error, or an
/// empty string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_ws_send(
    socket: *mut NativeObject,
    binary: u8,
    message: *const Str,
    data: *mut ArrayHeader,
) -> *mut Str {
    use futures_util::SinkExt;
    let Some(sink) = native::get::<Socket>(socket).and_then(|s| s.sink.clone()) else {
        return str_new(b"not an open WebSocket");
    };
    let frame = if binary != 0 { Frame::Binary(bytes_of(data).into()) } else { Frame::Text(text(message).into()) };
    match wait(async move { sink.lock().await.send(frame).await }) {
        Some(Ok(())) => str_new(b""),
        Some(Err(error)) => str_new(error.to_string().as_bytes()),
        None => str_new(b"the task was cancelled"),
    }
}

/// The next text or binary message; pings are answered and skipped, and a
/// close is the end.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_ws_receive(socket: *mut NativeObject) -> *mut NativeObject {
    use futures_util::StreamExt;
    use tokio_tungstenite::tungstenite::Error;
    let received =
        |kind: i64, text: String, data: Vec<u8>| native::new(Received { failure: Mutex::new(None), kind, text, data });
    let Some(stream) = native::get::<Socket>(socket).and_then(|s| s.stream.clone()) else {
        return received(2, String::new(), Vec::new());
    };
    let outcome = wait(async move {
        let mut stream = stream.lock().await;
        loop {
            match stream.next().await {
                Some(Ok(Frame::Text(text))) => return Ok((0, text.to_string(), Vec::new())),
                Some(Ok(Frame::Binary(data))) => return Ok((1, String::new(), data.to_vec())),
                Some(Ok(Frame::Close(_))) | None => return Ok((2, String::new(), Vec::new())),
                Some(Err(Error::ConnectionClosed | Error::AlreadyClosed)) => return Ok((2, String::new(), Vec::new())),
                Some(Err(error)) => return Err(ws_failure(error)),
                Some(Ok(_)) => {}
            }
        }
    });
    match outcome {
        Some(Ok((kind, text, data))) => received(kind, text, data),
        failed => {
            let failure = match failed {
                Some(Err(failure)) => failure,
                _ => ("protocol", "the task was cancelled".into()),
            };
            native::new(Received { failure: Mutex::new(Some(failure)), kind: 2, text: String::new(), data: Vec::new() })
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_ws_kind(received: *mut NativeObject) -> i64 {
    native::get::<Received>(received).map_or(2, |r| r.kind)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_ws_text(received: *mut NativeObject) -> *mut Str {
    str_new(native::get::<Received>(received).map_or("", |r| r.text.as_str()).as_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_ws_data(received: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(native::get::<Received>(received).map_or(&[][..], |r| r.data.as_slice()))
}

/// Closes a WebSocket, telling the server; the error, or an empty string.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_ws_close(socket: *mut NativeObject) -> *mut Str {
    use futures_util::SinkExt;
    let Some(sink) = native::get::<Socket>(socket).and_then(|s| s.sink.clone()) else {
        return str_new(b"");
    };
    match wait(async move { sink.lock().await.close().await }) {
        Some(Ok(())) | Some(Err(tokio_tungstenite::tungstenite::Error::ConnectionClosed)) => str_new(b""),
        Some(Err(error)) => str_new(error.to_string().as_bytes()),
        None => str_new(b"the task was cancelled"),
    }
}
