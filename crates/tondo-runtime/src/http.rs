//! What `std.http` does in the runtime (standard library 14.24): requests
//! over HTTP/1.1 and HTTP/2, with TLS, on a tokio runtime of its own. A task
//! waits for a request without holding a worker; a response's body reads
//! as a stream of `std.io`.

use super::native::{self, NativeObject};
use super::tasks::{CANCELLED, HOST_WORK, Wait, current, notify};
use super::{ArrayHeader, Str, element, str_new, texts_array};
use crate::array_items;
use crate::byte_array;
use crate::io::{Host, Mode, StreamObject};
use bytes::{Buf, Bytes};
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// The runtime, once a call needs it, until a library's end.
static RUNTIME: std::sync::atomic::AtomicPtr<tokio::runtime::Runtime> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());
/// What `SIGPIPE` did before the runtime ignored it.
static PIPE_BEFORE: Mutex<Option<usize>> = Mutex::new(None);

unsafe extern "C" {
    fn signal(signal: i32, handler: usize) -> usize;
}
const SIGPIPE: i32 = 13;
const SIG_IGN: usize = 1;

pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    if let Some(runtime) = unsafe { RUNTIME.load(Ordering::Acquire).as_ref() } {
        return runtime;
    }
    static STARTING: Mutex<()> = Mutex::new(());
    let _starting = STARTING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runtime) = unsafe { RUNTIME.load(Ordering::Acquire).as_ref() } {
        return runtime;
    }
    // A write to a connection the other side closed, which `writev`
    // makes without `MSG_NOSIGNAL`, fails instead of ending the program,
    // as in Rust programs; console programs keep the default.
    *PIPE_BEFORE.lock().unwrap_or_else(|e| e.into_inner()) = Some(unsafe { signal(SIGPIPE, SIG_IGN) });
    // A recorded run, and its replay, call the system from one thread:
    // its worker drives this runtime between tasks, and waits in it,
    // with the workers' poller among what it waits for.
    let mut builder = match super::replay::on() {
        true => {
            super::poll::started();
            tokio::runtime::Builder::new_current_thread()
        }
        // Half the cores, two at least: with 64 clients at once on 32
        // threads, two took 160 ms for 6,400 requests, half 90 and all
        // of them 89, the same as half, which leaves the rest to the
        // program's workers; one client is as quick with any.
        false => {
            let workers = std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).max(2));
            let mut builder = tokio::runtime::Builder::new_multi_thread();
            builder.worker_threads(workers).thread_name("tondo-http");
            builder
        }
    };
    builder.enable_all();
    // Built on a thread of the runtime: tokio starts its threads with std's
    // `spawn`, which in a library would keep it loaded while the thread of
    // its host that asked first lives.
    let built = Arc::new(Mutex::new(None));
    let given = built.clone();
    super::tasks::spawn_thread(None, move || {
        *given.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder.build().expect("the HTTP runtime starts"));
    })
    .join();
    let runtime = built.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the HTTP runtime starts");
    let runtime = Box::into_raw(Box::new(runtime));
    RUNTIME.store(runtime, Ordering::Release);
    unsafe { &*runtime }
}

/// The runtime the worker of a recorded run or of its replay drives, once
/// the program has one.
pub(crate) fn driven() -> Option<&'static tokio::runtime::Runtime> {
    unsafe { RUNTIME.load(Ordering::Acquire).as_ref() }.filter(|_| super::replay::on())
}

/// At a library's end, once nothing of the program uses the runtime: it
/// ends, its threads with it, and `SIGPIPE` does what it did before.
pub(crate) fn stop() {
    let runtime = RUNTIME.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if runtime.is_null() {
        return;
    }
    // Dropping it waits for every thread of it to end; on a thread of the
    // runtime, as it keeps thread-locals that drop where it waits.
    let runtime = unsafe { Box::from_raw(runtime) };
    super::tasks::spawn_thread(None, move || drop(runtime)).join();
    if let Some(before) = PIPE_BEFORE.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let current = unsafe { signal(SIGPIPE, before) };
        if current != SIG_IGN {
            unsafe { signal(SIGPIPE, current) };
        }
    }
}

/// Runs what the driven runtime has to do now, on this thread; with
/// `wait`, until the workers' poller has events, as when a task of the
/// program is woken, or `until`.
pub(crate) fn drive(runtime: &'static tokio::runtime::Runtime, wait: bool, until: Option<std::time::Instant>) {
    struct Raw(std::os::fd::RawFd);
    impl std::os::fd::AsRawFd for Raw {
        fn as_raw_fd(&self) -> std::os::fd::RawFd {
            self.0
        }
    }
    static POLLER: OnceLock<Option<tokio::io::unix::AsyncFd<Raw>>> = OnceLock::new();
    if !wait {
        runtime.block_on(tokio::task::yield_now());
        return;
    }
    runtime.block_on(async {
        let poller = POLLER.get_or_init(|| {
            let fd = super::poll::poller()?.fd();
            // The poller's, open for as long as the program runs.
            unsafe { tokio::io::unix::AsyncFd::register_with_interest(Raw(fd), tokio::io::Interest::READABLE) }.ok()
        });
        let ready = async {
            match poller {
                Some(poller) => {
                    if let Ok(mut ready) = poller.readable().await {
                        ready.clear_ready();
                    }
                }
                None => std::future::pending().await,
            }
        };
        match until {
            Some(at) => {
                tokio::select! {
                    _ = ready => {}
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(at)) => {}
                }
            }
            None => ready.await,
        }
    });
}

/// Work that blocks, the runtime's own, done where it holds no worker: on a
/// thread of the runtime's pool, `long` when it may wait indefinitely, as
/// reading a pipe does, or by the worker of a recorded run, whose calls the
/// recording has, between its tasks.
pub(crate) async fn blocking<T: Send + 'static>(
    long: bool,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<T> {
    let (done, result) = tokio::sync::oneshot::channel();
    let job = Box::new(move || {
        let _ = done.send(work());
    });
    match driven() {
        None => super::io::submit(job, long),
        Some(_) => {
            DEFERRED.lock().unwrap_or_else(|e| e.into_inner()).push_back(job);
            // The worker may wait in the runtime.
            if let Some(poller) = super::poll::poller() {
                poller.wake();
            }
        }
    }
    result.await.map_err(std::io::Error::other)
}

#[allow(clippy::type_complexity)]
static DEFERRED: Mutex<std::collections::VecDeque<Box<dyn FnOnce() + Send>>> =
    Mutex::new(std::collections::VecDeque::new());

/// The work that blocks the runtime gave the worker of a recorded run,
/// done now.
pub(crate) fn deferred() {
    loop {
        let Some(work) = DEFERRED.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { return };
        work();
    }
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
        // A driven runtime drops the future when this thread drives it.
        if let Some(runtime) = driven() {
            drop(over);
            drive(runtime, false, None);
            over = ended.0.lock().unwrap_or_else(|e| e.into_inner());
            continue;
        }
        over = ended.1.wait(over).unwrap_or_else(|e| e.into_inner());
    }
    drop(over);
    if cancelled {
        return None;
    }
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

pub(crate) fn text(s: *const Str) -> String {
    unsafe { crate::str_text(s) }.to_owned()
}

pub(crate) fn strings(array: *mut ArrayHeader) -> Vec<String> {
    if array.is_null() {
        return Vec::new();
    }
    let count = unsafe { (*array).len };
    (0..count).map(|i| text(unsafe { *(element(array, 8, i) as *const *const Str) })).collect()
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

/// A client's TLS: the roots and the authorities of `trust`, the
/// certificate and key it shows when `cert` is not empty, each a PEM's text
/// or file, and the lowest version it accepts; ALPN offers HTTP/2 first.
pub(crate) fn tls(trust: &str, cert: &str, key: &str, minimum: i64) -> Result<rustls::ClientConfig, String> {
    use super::server::{certificates, private_key, versions};
    let mut roots = roots();
    if !trust.is_empty() {
        for cert in certificates(trust, "the trusted certificates")? {
            roots.add(cert).map_err(|error| format!("the trusted certificates: {error}"))?;
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(versions(minimum))
        .map_err(|error| error.to_string())?
        .with_root_certificates(roots);
    let mut config = match cert.is_empty() {
        true => builder.with_no_client_auth(),
        false => builder
            .with_client_auth_cert(certificates(cert, "the certificate")?, private_key(key)?)
            .map_err(|error| error.to_string())?,
    };
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(config)
}

/// A client: its connections, reused by its requests, or why it has none.
/// `redirects` is how many it follows; a connection waits `connect_ms`, or
/// forever with 0; `trust`, `cert`, `key` and `minimum` are its TLS's.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_client(
    connect_ms: i64,
    redirects: i64,
    trust: *const Str,
    socket: *const Str,
    cookies: u8,
    cert: *const Str,
    key: *const Str,
    minimum: i64,
) -> *mut NativeObject {
    let made = tls(&text(trust), &text(cert), &text(key), minimum).map(|config| {
        let socket = Some(text(socket)).filter(|socket| !socket.is_empty()).map(std::path::PathBuf::from);
        let connect = (connect_ms > 0).then(|| Duration::from_millis(connect_ms as u64));
        Arc::new(super::client::Client::new(config, connect, redirects.max(0) as usize, socket, cookies != 0))
    });
    native::new(made)
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
type Form = Result<super::client::Form, String>;

/// The chunks a stream of `std.io` gives, read on blocking threads.
pub(crate) fn stream_of(
    source: Source,
) -> impl futures_util::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    futures_util::stream::unfold(Arc::new(source), |source| async move {
        // A pipe is read here, without a thread, so that a reply dropped
        // while it waits, as when its client goes, lets the pipe go too.
        let read = if crate::io::waits_without_a_thread(source.0) {
            loop {
                let (read, blocked) = crate::io::attempt(source.0, |s| s.read(64 * 1024));
                match blocked {
                    Some(blocked) => blocked.await,
                    None => break read,
                }
            }
        } else {
            let reading = source.clone();
            blocking(true, move || crate::io::run(reading.0, |s| s.read(64 * 1024))).await.and_then(|read| read)
        };
        match read {
            Ok(Some(bytes)) => Some((Ok(Bytes::from(bytes)), source)),
            Ok(None) => None,
            Err(error) => Some((Err(error), source)),
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
    native::new(Body::Bytes(Bytes::copy_from_slice(unsafe { array_items::<u8>(data) }), text(content_type)))
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
    native::new(Mutex::new(Some(Ok::<_, String>(super::client::Form::default()))))
}

/// Adds a part to a form under construction, with its file name and
/// content type when given; a content type that does not parse is the
/// form's problem.
fn add_part(form: *mut NativeObject, part: super::client::Part) {
    let Some(slot) = native::get::<Mutex<Option<Form>>>(form) else { return };
    let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
    let kind = &part.content_type;
    let wrong = !kind.is_empty() && kind.parse::<mime_guess::mime::Mime>().is_err();
    if let Some(Ok(current)) = slot.as_mut() {
        match wrong {
            true => *slot = Some(Err(format!("`{kind}` is not a content type"))),
            false => current.add(part),
        }
    }
}

fn part(name: String, file_name: String, content_type: String, content: super::client::Content) -> super::client::Part {
    super::client::Part { name, file_name, content_type, content }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_text(form: *mut NativeObject, name: *const Str, value: *const Str) {
    let value = super::client::Content::Bytes(Bytes::from(text(value)));
    add_part(form, part(text(name), String::new(), String::new(), value));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_form_bytes(
    form: *mut NativeObject,
    name: *const Str,
    data: *mut ArrayHeader,
    file_name: *const Str,
    content_type: *const Str,
) {
    let data = super::client::Content::Bytes(Bytes::copy_from_slice(unsafe { array_items::<u8>(data) }));
    add_part(form, part(text(name), text(file_name), text(content_type), data));
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
    let path = std::path::PathBuf::from(text(path));
    let opening = path.clone();
    let opened = super::io::wait_for(async move {
        blocking(false, move || {
            let file = std::fs::File::open(&opening)?;
            let length = file.metadata()?.len();
            Ok::<_, std::io::Error>((file, length))
        })
        .await?
    });
    match opened {
        Some(Ok((file, length))) => {
            // Its name, and the content type of its extension, unless given.
            let file_name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            let kind = match text(content_type) {
                given if given.is_empty() => mime_guess::from_path(&path).first_or_octet_stream().to_string(),
                given => given,
            };
            let content = super::client::Content::Stream(Box::pin(read_file(file)), Some(length));
            add_part(form, part(text(name), file_name, kind, content));
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
    let content = super::client::Content::Stream(Box::pin(stream_of(Source(input))), u64::try_from(length).ok());
    add_part(form, part(text(name), text(file_name), text(content_type), content));
}

/// A file's chunks, read on the runtime's pool as they are sent.
fn read_file(file: std::fs::File) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    futures_util::stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let read = blocking(false, move || {
            let mut buffer = vec![0; 64 * 1024];
            let count = std::io::Read::read(&mut file, &mut buffer)?;
            buffer.truncate(count);
            Ok::<_, std::io::Error>((file, buffer))
        })
        .await
        .and_then(|read| read);
        match read {
            Ok((_, buffer)) if buffer.is_empty() => None,
            Ok((file, buffer)) => Some((Ok(Bytes::from(buffer)), Some(file))),
            Err(error) => Some((Err(error), None)),
        }
    })
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
    url: String,
    version: hyper::Version,
    /// As they came, until the program asks for one.
    headers: hyper::HeaderMap,
    body: Mutex<Option<super::client::Received>>,
}

impl Response {
    fn failed(kind: &'static str, message: String) -> Response {
        Response {
            failure: Mutex::new(Some((kind, message))),
            status: 0,
            url: String::new(),
            version: hyper::Version::default(),
            headers: hyper::HeaderMap::new(),
            body: Mutex::new(None),
        }
    }
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
    let client = match native::get::<Result<Arc<super::client::Client>, String>>(client) {
        Some(Ok(client)) => client.clone(),
        Some(Err(problem)) => return native::new(Response::failed("connect", problem.clone())),
        None => return native::new(Response::failed("protocol", "not an HTTP client".into())),
    };
    let Ok(method) = hyper::Method::from_bytes(text(method).as_bytes()) else {
        return native::new(Response::failed("protocol", format!("`{}` is not an HTTP method", text(method))));
    };
    let headers: Vec<(String, String)> =
        strings(headers).chunks(2).filter_map(|pair| Some((pair.first()?.clone(), pair.get(1)?.clone()))).collect();
    use super::client::Body as Sent;
    let body = match native::get::<Body>(body) {
        None | Some(Body::Empty) => Sent::Empty,
        Some(Body::Bytes(data, content_type)) => Sent::Bytes(data.clone(), content_type.clone()),
        Some(Body::Stream(source, length, content_type)) => {
            let Some(source) = source.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                return native::new(Response::failed("body", "a stream body is sent once".into()));
            };
            let chunks = super::client::streamed(stream_of(source));
            Sent::Stream(chunks, u64::try_from(*length).ok(), content_type.clone())
        }
        Some(Body::Form(form)) => match form.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(form)) => Sent::Form(form),
            Some(Err(problem)) => return native::new(Response::failed("body", problem)),
            None => return native::new(Response::failed("body", "a multipart body is sent once".into())),
        },
    };
    let timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms as u64));
    let request = super::client::Request { method, url: text(url), headers, body, timeout };
    // Polled by the task in place, as the server's futures are; only the
    // futures of the connections run at the workers' turns.
    let response = match super::io::wait_for(async move { client.send(request).await }) {
        None => Response::failed("protocol", "the task was cancelled".into()),
        Some(Err((kind, message))) => Response::failed(kind, message),
        Some(Ok(answer)) => Response {
            failure: Mutex::new(None),
            status: answer.status.as_u16(),
            url: answer.url,
            version: answer.version,
            headers: answer.headers,
            body: Mutex::new(Some(answer.body)),
        },
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

/// The status's reason phrase: `OK`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_reason(object: *mut NativeObject) -> *mut Str {
    let reason = hyper::StatusCode::from_u16(response(object).status).ok().and_then(|s| s.canonical_reason());
    str_new(reason.unwrap_or_default().as_bytes())
}

/// The URL that answered, after redirects.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_final_url(object: *mut NativeObject) -> *mut Str {
    str_new(response(object).url.as_bytes())
}

/// The protocol's version: `HTTP/1.1`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_version(object: *mut NativeObject) -> *mut Str {
    super::server::version_text(response(object).version)
}

/// The values of a header, by its name in any case.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_header(object: *mut NativeObject, name: *const Str) -> *mut ArrayHeader {
    let values = response(object).headers.get_all(text(name).as_str());
    texts_array(values.iter().count(), values.iter().map(|value| value.as_bytes()))
}

/// The headers' names, lowercase, and values in turn.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_headers(object: *mut NativeObject) -> *mut ArrayHeader {
    let headers = &response(object).headers;
    let pairs = headers.iter().flat_map(|(name, value)| [name.as_str().as_bytes(), value.as_bytes()]);
    texts_array(headers.len() * 2, pairs)
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
    match super::io::wait_for(async move { body.all().await }) {
        Some(Ok(bytes)) => byte_array(&bytes),
        outcome => {
            let failure = match outcome {
                Some(Err(failure)) => failure,
                _ => ("protocol", "the task was cancelled".into()),
            };
            *response.failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(failure);
            byte_array(&[])
        }
    }
}

/// A response's body as the host of a stream: chunks as they arrive.
struct Chunks {
    body: Option<super::client::Received>,
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
                Err((_, message)) => return Err(std::io::Error::other(message)),
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
    str_new(windows_1252(unsafe { array_items::<u8>(data) }).as_bytes())
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
    let mut config = match tls("", "", "", 12) {
        Ok(config) => config,
        Err(problem) => return failed("connect", problem),
    };
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_tungstenite::Connector::Rustls(Arc::new(config));
    // Connected as `std.net` connects, which looks the host up on the
    // worker of a recorded run.
    let connecting = async move {
        use tokio_tungstenite::tungstenite::Error;
        let uri = request.uri();
        let host = uri.host().unwrap_or_default().trim_start_matches('[').trim_end_matches(']').to_string();
        let port = uri.port_u16().unwrap_or(if uri.scheme_str() == Some("wss") { 443 } else { 80 });
        let mut last = std::io::Error::other(format!("{host} has no address"));
        let mut connected = None;
        for address in crate::net::addresses(&host, port).await.map_err(Error::Io)? {
            match tokio::net::TcpStream::connect(address).await {
                Ok(socket) => {
                    connected = Some(socket);
                    break;
                }
                Err(error) => last = error,
            }
        }
        let socket = connected.ok_or(Error::Io(last))?;
        tokio_tungstenite::client_async_tls_with_config(request, socket, None, Some(connector)).await
    };
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
    let frame = if binary != 0 {
        Frame::Binary(unsafe { array_items::<u8>(data) }.to_vec().into())
    } else {
        Frame::Text(text(message).into())
    };
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
