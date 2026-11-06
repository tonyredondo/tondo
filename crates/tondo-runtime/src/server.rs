//! The server of `std.http` (standard library 14.24): hyper accepts
//! connections, over TLS with a certificate, and hands each request to Tondo
//! through a channel; the task that handles it sends the response back.
//! Files go out with ranges, so a download can resume, and uploads go to
//! disk as they arrive.

use super::http::{Failure, Source, blocking, bytes_of, runtime, socket, stream_of, string_array, strings, text, wait};
use super::native::{self, NativeObject};
use super::{ArrayHeader, Str, str_new};
use crate::io::{Host, Mode, StreamObject, byte_array};
use bytes::{Buf, Bytes};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use hyper::body::{Frame, Incoming};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, watch};

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

struct Server {
    failure: Failure,
    address: String,
    requests: Option<Arc<tokio::sync::Mutex<mpsc::Receiver<Request>>>>,
    life: Option<Arc<Life>>,
    /// The file of a server on a Unix socket, which goes when it stops.
    socket: Option<String>,
}

/// Where a server is in its life, how many connections it has open, and
/// whether it still listens for more.
struct Life {
    phase: watch::Sender<Phase>,
    open: watch::Sender<usize>,
    listening: watch::Sender<bool>,
}

/// Serving; after `stop`, draining, as its connections end the requests
/// they have and close; and closed, once they all did or its grace passed.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Serving,
    Draining,
    Closed,
}

/// A connection the server counts while it is open.
struct Counted(Arc<Life>);

impl Counted {
    fn new(life: Arc<Life>) -> Counted {
        life.open.send_modify(|open| *open += 1);
        Counted(life)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.open.send_modify(|open| *open -= 1);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(path) = &self.socket {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// What a server accepts connections on: a TCP port or a Unix socket.
enum Listening {
    Tcp(tokio::net::TcpListener),
    Unix(tokio::net::UnixListener),
}

/// A connection a server accepted, of either kind.
trait Connected: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static {}

impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static> Connected for T {}

impl Listening {
    /// The next connection, and who it is from.
    async fn accept(&self) -> std::io::Result<(Box<dyn Connected>, String)> {
        match self {
            Listening::Tcp(listener) => {
                let (stream, remote) = listener.accept().await?;
                Ok((Box::new(stream), remote.to_string()))
            }
            Listening::Unix(listener) => Ok((Box::new(listener.accept().await?.0), String::new())),
        }
    }
}

/// What a server allows each client (`http.Limits`): the bytes of a
/// request's body and of its head, the time to send a head, which also
/// ends a connection kept open without a request, and the longest a body
/// may stop arriving, and a client stop taking a reply.
#[derive(Clone, Copy)]
struct Limits {
    body: usize,
    head: usize,
    head_timeout: std::time::Duration,
    body_timeout: std::time::Duration,
    reply_timeout: std::time::Duration,
}

impl Limits {
    fn of(body: i64, head: i64, timeouts: [i64; 3]) -> Limits {
        let millis = |ms: i64| std::time::Duration::from_millis(ms.max(1) as u64);
        Limits {
            body: body.max(0) as usize,
            // hyper reads a head in a buffer of at least 8 KiB.
            head: head.max(8192) as usize,
            head_timeout: millis(timeouts[0]),
            body_timeout: millis(timeouts[1]),
            reply_timeout: millis(timeouts[2]),
        }
    }
}

/// A request's body that fails when none of it arrives for `idle`
/// (`http.Limits.bodyTimeout`), and marks `stalled`.
struct Idle {
    inner: Incoming,
    idle: std::time::Duration,
    sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    stalled: Arc<std::sync::atomic::AtomicBool>,
}

impl hyper::body::Body for Idle {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        use std::future::Future;
        use std::task::Poll;
        let this = &mut *self;
        match std::pin::Pin::new(&mut this.inner).poll_frame(cx) {
            Poll::Ready(frame) => {
                this.sleep = None;
                Poll::Ready(frame.map(|frame| frame.map_err(Into::into)))
            }
            Poll::Pending => {
                let idle = this.idle;
                let sleep = this.sleep.get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
                match sleep.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        this.sleep = None;
                        this.stalled.store(true, std::sync::atomic::Ordering::Release);
                        let message = format!("the body stopped arriving for {} seconds", idle.as_secs());
                        Poll::Ready(Some(Err(message.into())))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

/// A connection whose writes fail when the client takes none of the reply
/// for `idle` (`http.Limits.replyTimeout`): hyper then closes it, and the
/// handler's task is cancelled as for a client gone.
struct Unhurried<S> {
    inner: S,
    idle: std::time::Duration,
    sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
}

impl<S> Unhurried<S> {
    fn waited<T>(
        &mut self,
        cx: &mut std::task::Context<'_>,
        polled: std::task::Poll<std::io::Result<T>>,
    ) -> std::task::Poll<std::io::Result<T>> {
        use std::future::Future;
        use std::task::Poll;
        match polled {
            Poll::Ready(done) => {
                self.sleep = None;
                Poll::Ready(done)
            }
            Poll::Pending => {
                let idle = self.idle;
                let sleep = self.sleep.get_or_insert_with(|| Box::pin(tokio::time::sleep(idle)));
                match sleep.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        self.sleep = None;
                        Poll::Ready(Err(std::io::ErrorKind::TimedOut.into()))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
        }
    }
}

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Unhurried<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Unhurried<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_write(cx, buf);
        self.waited(cx, polled)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_flush(cx);
        self.waited(cx, polled)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let polled = std::pin::Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.waited(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

/// Who answers a request: the task, which goes on only while its client
/// waits, whether the response went out, and whether the client is gone.
#[derive(Default)]
struct Answering {
    task: std::sync::atomic::AtomicU64,
    answered: std::sync::atomic::AtomicBool,
    gone: std::sync::atomic::AtomicBool,
}

impl Answering {
    /// Cancels the task when its client went away before the response.
    /// Both the task, when it starts, and the request's future, when it
    /// ends, call it, so the second does it even if the client left while
    /// the request waited for a task.
    fn cancel_if_gone(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        let task = self.task.load(SeqCst);
        if task != 0 && self.gone.load(SeqCst) && !self.answered.load(SeqCst) {
            super::tasks::cancel_by_id(task);
        }
    }
}

/// Marks the client of a request gone when hyper drops the request's
/// future: after the response, or when the client went away first.
struct Abandoned(Arc<Answering>);

impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.gone.store(true, std::sync::atomic::Ordering::SeqCst);
        self.0.cancel_if_gone();
    }
}

/// Marks `exceeded` when a body failed to read because it went past its
/// limit.
fn note_exceeded(error: &(dyn std::error::Error + 'static), exceeded: &std::sync::atomic::AtomicBool) {
    if error.is::<http_body_util::LengthLimitError>() {
        exceeded.store(true, std::sync::atomic::Ordering::Release);
    }
}

/// A request waiting for its response; `open` is false for the mark that
/// the server stopped.
struct Request {
    failure: Failure,
    open: bool,
    /// While a dump would show it.
    flight: Option<super::diagnostics::Flown>,
    method: String,
    path: String,
    query: String,
    version: String,
    remote: String,
    /// The host the request was for: `:authority`, or `Host`.
    authority: String,
    /// Whether it came over TLS.
    secure: bool,
    /// The DER of the certificate the client showed, if it showed one.
    client: Arc<Vec<u8>>,
    headers: Vec<String>,
    body: Mutex<Option<Incoming>>,
    /// Whether the body was taken to be read, which happens once.
    taken: std::sync::atomic::AtomicBool,
    /// The bytes its body may have, which the program may change before
    /// it reads it, and whether a reading went past them.
    limit: std::sync::atomic::AtomicUsize,
    exceeded: Arc<std::sync::atomic::AtomicBool>,
    /// The longest its body may stop arriving, and whether it did.
    body_timeout: std::time::Duration,
    stalled: Arc<std::sync::atomic::AtomicBool>,
    reply: Mutex<Option<oneshot::Sender<hyper::Response<Body>>>>,
    upgrade: Mutex<Option<hyper::upgrade::OnUpgrade>>,
    answering: Arc<Answering>,
}

impl Request {
    fn closed() -> Request {
        Request {
            flight: None,
            failure: Mutex::new(None),
            open: false,
            method: String::new(),
            path: String::new(),
            query: String::new(),
            version: String::new(),
            remote: String::new(),
            authority: String::new(),
            secure: false,
            client: Arc::default(),
            headers: Vec::new(),
            body: Mutex::new(None),
            taken: Default::default(),
            limit: Default::default(),
            exceeded: Arc::default(),
            body_timeout: std::time::Duration::MAX,
            stalled: Arc::default(),
            reply: Mutex::new(None),
            upgrade: Mutex::new(None),
            answering: Arc::default(),
        }
    }

    /// The body, once, held to its limit and its time.
    fn take_body(&self) -> Option<http_body_util::Limited<Idle>> {
        let limit = self.limit.load(std::sync::atomic::Ordering::Acquire);
        self.taken.store(true, std::sync::atomic::Ordering::Release);
        lock(&self.body).take().map(|body| {
            let idle = Idle { inner: body, idle: self.body_timeout, sleep: None, stalled: self.stalled.clone() };
            http_body_util::Limited::new(idle, limit)
        })
    }

    /// Records a failure to read the body: past its limit, or another.
    fn body_failed(&self, error: &(dyn std::error::Error + 'static)) -> String {
        note_exceeded(error, &self.exceeded);
        error.to_string()
    }

    /// The first value of a header, by its lowercase name.
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.chunks(2).find(|pair| pair[0] == name).map(|pair| pair[1].as_str())
    }

    fn send(&self, response: hyper::Response<Body>) {
        self.answering.answered.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(reply) = lock(&self.reply).take() {
            let _ = reply.send(response);
        }
    }
}

/// Where an object of the server keeps its failure.
pub(crate) fn failure_of<'a>(object: *mut NativeObject) -> Option<&'a Failure> {
    native::get::<Server>(object)
        .map(|s| &s.failure)
        .or_else(|| native::get::<Request>(object).map(|r| &r.failure))
        .or_else(|| native::get::<Form>(object).map(|f| &f.failure))
        .or_else(|| native::get::<Upload>(object).map(|u| &u.failure))
}

fn request<'a>(object: *mut NativeObject) -> &'a Request {
    native::get::<Request>(object).expect("an HTTP request")
}

fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into()).map_err(|never| match never {}).boxed_unsync()
}

fn plain(status: u16, message: &str) -> hyper::Response<Body> {
    let mut response = hyper::Response::new(full(message.to_string()));
    *response.status_mut() = hyper::StatusCode::from_u16(status).unwrap_or(hyper::StatusCode::INTERNAL_SERVER_ERROR);
    response
}

/// A response with the status and headers, names and values in turn.
fn response(status: i64, headers: &[String], body: Body) -> hyper::Response<Body> {
    let mut builder = hyper::Response::builder().status(u16::try_from(status).unwrap_or(500));
    for pair in headers.chunks(2) {
        if let [name, value] = pair {
            builder = builder.header(name.as_str(), value.as_str());
        }
    }
    builder.body(body).unwrap_or_else(|error| plain(500, &format!("a header is not valid: {error}")))
}

/// The bytes of a PEM given as its text, which starts with `-----BEGIN`,
/// or as the path of its file (standard library 14.4.6).
pub(crate) fn pem(given: &str, what: &str) -> Result<Vec<u8>, String> {
    if given.trim_start().starts_with("-----BEGIN") {
        return Ok(given.as_bytes().to_vec());
    }
    std::fs::read(given).map_err(|error| format!("{what} `{given}`: {error}"))
}

/// What a PEM was given as, for an error: its path, but never its text.
fn shown(given: &str) -> String {
    match given.trim_start().starts_with("-----BEGIN") {
        true => String::new(),
        false => format!(" `{given}`"),
    }
}

/// The versions of TLS from the lowest one allowed, 1.2 (12) or 1.3 (13).
pub(crate) fn versions(minimum: i64) -> &'static [&'static rustls::SupportedProtocolVersion] {
    static ONLY_13: [&rustls::SupportedProtocolVersion; 1] = [&rustls::version::TLS13];
    match minimum {
        13 => &ONLY_13,
        _ => rustls::DEFAULT_VERSIONS,
    }
}

/// The certificates of a PEM, given as its text or its file.
pub(crate) fn certificates(given: &str, what: &str) -> Result<Vec<rustls_pki_types::CertificateDer<'static>>, String> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    let certs = CertificateDer::pem_slice_iter(&pem(given, what)?)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{what}{}: {error}", shown(given)))?;
    match certs.is_empty() {
        true => Err(format!("{what}{} holds no `CERTIFICATE`", shown(given))),
        false => Ok(certs),
    }
}

/// The private key of a PEM, given as its text or its file.
pub(crate) fn private_key(given: &str) -> Result<rustls_pki_types::PrivateKeyDer<'static>, String> {
    use rustls_pki_types::PrivateKeyDer;
    use rustls_pki_types::pem::PemObject;
    let bytes = pem(given, "the private key")?;
    PrivateKeyDer::from_pem_slice(&bytes).map_err(|error| format!("the private key{}: {error}", shown(given)))
}

/// The TLS of a server: its certificate chain and private key, the
/// authorities whose client certificates it requires when `clients` is not
/// empty, each a PEM's text or file, and the lowest version it accepts;
/// ALPN offers HTTP/2 first when `http`.
pub(crate) fn acceptor(
    cert: &str,
    key: &str,
    clients: &str,
    minimum: i64,
    http: bool,
) -> Result<tokio_rustls::TlsAcceptor, String> {
    let changing = store(cert);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions(minimum))
        .map_err(|error| error.to_string())?;
    let builder = match clients.is_empty() {
        true => builder.with_no_client_auth(),
        false => {
            let mut roots = rustls::RootCertStore::empty();
            for authority in certificates(clients, "the authorities of the clients")? {
                roots.add(authority).map_err(|error| format!("the authorities of the clients: {error}"))?;
            }
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .map_err(|error| format!("the authorities of the clients: {error}"))?;
            builder.with_client_cert_verifier(verifier)
        }
    };
    let mut config = match &changing {
        Some(store) => {
            store.serving.store(true, std::sync::atomic::Ordering::Release);
            builder.with_cert_resolver(Arc::new(Resolver(store.clone())))
        }
        None => builder
            .with_single_cert(certificates(cert, "the certificate")?, private_key(key)?)
            .map_err(|error| error.to_string())?,
    };
    if http {
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    }
    // The protocols certificates are kept for, as `acme-tls/1`, last.
    if let Some(store) = &changing {
        for protocol in &store.offered {
            if !config.alpn_protocols.contains(protocol) {
                config.alpn_protocols.push(protocol.clone());
            }
        }
    }
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The certificates of `net.Certificates`, which change while a server
/// shows them: the one it shows, and those kept for a protocol a client
/// asks for and a name, as an ACME validation asks for `acme-tls/1`, which
/// the server offers.
pub(crate) struct Certificates {
    current: Mutex<Option<Certified>>,
    /// By protocol and server name.
    protocols: Mutex<HashMap<(Vec<u8>, String), Certified>>,
    offered: Vec<Vec<u8>>,
    serving: std::sync::atomic::AtomicBool,
}

impl std::fmt::Debug for Certificates {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("Certificates")
    }
}

type Certified = Arc<rustls::sign::CertifiedKey>;

#[derive(Debug)]
struct Resolver(Arc<Certificates>);

impl rustls::server::ResolvesServerCert for Resolver {
    fn resolve(&self, hello: rustls::server::ClientHello) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let name = hello.server_name().unwrap_or_default().to_ascii_lowercase();
        if let Some(asked) = hello.alpn() {
            let kept = lock(&self.0.protocols);
            for protocol in asked {
                if let Some(found) = kept.get(&(protocol.to_vec(), name.clone())) {
                    return Some(found.clone());
                }
            }
        }
        lock(&self.0.current).clone()
    }
}

fn certified(cert: &str, key: &str) -> Result<Arc<rustls::sign::CertifiedKey>, String> {
    let chain = certificates(cert, "the certificate")?;
    let signing = rustls::crypto::ring::sign::any_supported_type(&private_key(key)?)
        .map_err(|error| format!("the private key: {error}"))?;
    Ok(Arc::new(rustls::sign::CertifiedKey::new(chain, signing)))
}

/// Every store of certificates, which a `Tls` names in its certificate as
/// `tondo:certificates:` and its number; they live as long as the program.
static STORES: Mutex<Vec<Arc<Certificates>>> = Mutex::new(Vec::new());
const STORE: &str = "tondo:certificates:";

/// The store a certificate names, if it names one.
fn store(name: &str) -> Option<Arc<Certificates>> {
    let index: usize = name.strip_prefix(STORE)?.parse().ok()?;
    lock(&STORES).get(index).cloned()
}

/// `net.Certificates.new(protocols)`: a new store, and its name.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_tls_certificates(protocols: *mut ArrayHeader) -> *mut Str {
    let offered = strings(protocols).into_iter().map(String::into_bytes).collect();
    let mut stores = lock(&STORES);
    stores.push(Arc::new(Certificates {
        current: Mutex::new(None),
        protocols: Mutex::new(HashMap::new()),
        offered,
        serving: Default::default(),
    }));
    str_new(format!("{STORE}{}", stores.len() - 1).as_bytes())
}

/// The certificate a store shows from now on, or, with a `protocol`, the
/// one it shows to a client that asks for it and for `name`; an empty
/// certificate forgets that one. Gives why it could not, or nothing.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_tls_certificates_set(
    named: *const Str,
    protocol: *const Str,
    name: *const Str,
    cert: *const Str,
    key: *const Str,
) -> *mut Str {
    let Some(store) = store(&text(named)) else { return str_new(b"") };
    let (protocol, name, cert) = (text(protocol).into_bytes(), text(name).to_ascii_lowercase(), text(cert));
    if protocol.is_empty() {
        return match certified(&cert, &text(key)) {
            Ok(found) => {
                *lock(&store.current) = Some(found);
                str_new(b"")
            }
            Err(why) => str_new(why.as_bytes()),
        };
    }
    if cert.is_empty() {
        lock(&store.protocols).remove(&(protocol, name));
        return str_new(b"");
    }
    if !store.offered.contains(&protocol) {
        let protocol = String::from_utf8_lossy(&protocol);
        let why = format!(
            "these certificates offer no `{protocol}`: make them with `Certificates.new(protocols: [\"{protocol}\"])`"
        );
        return str_new(why.as_bytes());
    }
    match certified(&cert, &text(key)) {
        Ok(found) => {
            lock(&store.protocols).insert((protocol, name), found);
            str_new(b"")
        }
        Err(why) => str_new(why.as_bytes()),
    }
}

/// Whether a server shows the store.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_tls_certificates_serving(named: *const Str) -> u8 {
    store(&text(named)).is_some_and(|store| store.serving.load(std::sync::atomic::Ordering::Acquire)) as u8
}

/// Starts a server on `host:port`, `0` for any free port, over TLS when it
/// has a certificate and a key, as `acceptor` takes them.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn tondo_rt_http_listen(
    host: *const Str,
    port: i64,
    cert: *const Str,
    key: *const Str,
    clients: *const Str,
    minimum: i64,
    body: i64,
    head: i64,
    head_timeout_ms: i64,
    body_timeout_ms: i64,
    reply_timeout_ms: i64,
) -> *mut NativeObject {
    let limits = Limits::of(body, head, [head_timeout_ms, body_timeout_ms, reply_timeout_ms]);
    let (cert, key, clients) = (text(cert), text(key), text(clients));
    let tls = match cert.is_empty() {
        true => None,
        false => match acceptor(&cert, &key, &clients, minimum, true) {
            Ok(acceptor) => Some(acceptor),
            Err(problem) => return failed(problem),
        },
    };
    let address = format!("{}:{port}", text(host));
    let listener = match wait(async move { tokio::net::TcpListener::bind(address).await }) {
        Some(Ok(listener)) => listener,
        Some(Err(error)) => return failed(error.to_string()),
        None => return failed("the task was cancelled".into()),
    };
    let address = listener.local_addr().map(|a| a.to_string()).unwrap_or_default();
    started(Listening::Tcp(listener), tls, address, None, limits)
}

/// The media type a file's name says, or `application/octet-stream`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_media_type(path: *const Str) -> *mut Str {
    str_new(mime_guess::from_path(text(path)).first_or_octet_stream().as_ref().as_bytes())
}

/// Starts a server on the Unix socket at `path`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_listen_unix(
    path: *const Str,
    body: i64,
    head: i64,
    head_timeout_ms: i64,
    body_timeout_ms: i64,
    reply_timeout_ms: i64,
) -> *mut NativeObject {
    let (path, limits) = (text(path), Limits::of(body, head, [head_timeout_ms, body_timeout_ms, reply_timeout_ms]));
    if let Err(problem) = super::net::make_room(&path, false) {
        return failed(problem.to_string());
    }
    let listener = {
        let _context = runtime().enter();
        tokio::net::UnixListener::bind(&path)
    };
    match listener {
        Ok(listener) => started(Listening::Unix(listener), None, path.clone(), Some(path), limits),
        Err(error) => failed(error.to_string()),
    }
}

/// A server that could not start, and why.
fn failed(message: String) -> *mut NativeObject {
    native::new(Server {
        failure: Mutex::new(Some(("listen", message))),
        address: String::new(),
        requests: None,
        life: None,
        socket: None,
    })
}

/// A server accepting on `listening` in a task of its own.
fn started(
    listening: Listening,
    tls: Option<tokio_rustls::TlsAcceptor>,
    address: String,
    socket: Option<String>,
    limits: Limits,
) -> *mut NativeObject {
    let (sender, receiver) = mpsc::channel(1024);
    let life = Arc::new(Life {
        phase: watch::channel(Phase::Serving).0,
        open: watch::channel(0).0,
        listening: watch::channel(true).0,
    });
    // Watched from now, so that a `stop` before the task first runs is seen.
    let stopped = life.phase.subscribe();
    runtime().spawn(accept(listening, tls, sender, life.clone(), stopped, limits));
    native::new(Server {
        failure: Mutex::new(None),
        address,
        requests: Some(Arc::new(tokio::sync::Mutex::new(receiver))),
        life: Some(life),
        socket,
    })
}

async fn accept(
    listener: Listening,
    tls: Option<tokio_rustls::TlsAcceptor>,
    sender: mpsc::Sender<Request>,
    life: Arc<Life>,
    mut stopped: watch::Receiver<Phase>,
    limits: Limits,
) {
    use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
    loop {
        let (stream, remote) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(accepted) => accepted,
                Err(_) => continue,
            },
            _ = stopped.changed() => {
                // The port is free once `stop` returns, for a server that
                // takes it again or a client that must find it closed.
                drop(listener);
                life.listening.send_replace(false);
                return;
            }
        };
        let (sender, tls, mut stopping) = (sender.clone(), tls.clone(), stopped.clone());
        let counted = Counted::new(life.clone());
        tokio::spawn(async move {
            let _counted = counted;
            let secure = tls.is_some();
            let (stream, client): (Box<dyn Connected>, _) = match tls {
                Some(acceptor) => {
                    // A client that does not finish its handshake in time is let go.
                    let shaken = tokio::time::timeout(limits.head_timeout, acceptor.accept(stream)).await;
                    let Ok(Ok(stream)) = shaken else { return };
                    let shown = stream.get_ref().1.peer_certificates().and_then(|c| c.first()).map(|c| c.to_vec());
                    (Box::new(stream), shown.unwrap_or_default())
                }
                None => (stream, Vec::new()),
            };
            let (client, answering) = (Arc::new(client), stopping.clone());
            let serve = hyper::service::service_fn(move |request| {
                handle(request, sender.clone(), remote.clone(), secure, client.clone(), limits, answering.clone())
            });
            let mut builder = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
            builder.http1().timer(TokioTimer::new()).header_read_timeout(limits.head_timeout).max_buf_size(limits.head);
            builder.http2().max_header_list_size(limits.head.min(u32::MAX as usize) as u32);
            let stream = Unhurried { inner: stream, idle: limits.reply_timeout, sleep: None };
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), serve);
            tokio::pin!(connection);
            // When the server stops, a connection kept open takes no more
            // requests: it ends those it has and closes, with a GOAWAY on
            // HTTP/2, so that its client connects again; once the server's
            // grace passes, it closes as it is.
            tokio::select! {
                _ = connection.as_mut() => {}
                _ = async { _ = stopping.wait_for(|phase| *phase != Phase::Serving).await } => {
                    connection.as_mut().graceful_shutdown();
                    tokio::select! {
                        _ = connection.as_mut() => {}
                        _ = async {
                            if stopping.wait_for(|phase| *phase == Phase::Closed).await.is_err() {
                                std::future::pending::<()>().await;
                            }
                        } => {}
                    }
                }
            }
        });
    }
}

async fn handle(
    mut request: hyper::Request<Incoming>,
    sender: mpsc::Sender<Request>,
    remote: String,
    secure: bool,
    client: Arc<Vec<u8>>,
    limits: Limits,
    stopped: watch::Receiver<Phase>,
) -> Result<hyper::Response<Body>, Infallible> {
    if *stopped.borrow() == Phase::Closed {
        return Ok(plain(503, "the server is stopping"));
    }
    super::diagnostics::HTTP_SERVER_REQUESTS.add(1);
    let _active = super::diagnostics::Open::new(&super::diagnostics::HTTP_SERVER_ACTIVE);
    let websocket = request
        .headers()
        .get(hyper::header::UPGRADE)
        .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"));
    let upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    let (parts, body) = request.into_parts();
    let authority = match parts.uri.authority() {
        Some(authority) => authority.to_string(),
        None => parts
            .headers
            .get(hyper::header::HOST)
            .map_or(String::new(), |h| String::from_utf8_lossy(h.as_bytes()).into_owned()),
    };
    let headers = parts
        .headers
        .iter()
        .flat_map(|(name, value)| [name.to_string(), String::from_utf8_lossy(value.as_bytes()).into_owned()])
        .collect();
    let (reply, answer) = oneshot::channel();
    let incoming = Request {
        failure: Mutex::new(None),
        open: true,
        flight: super::diagnostics::Flown::new(parts.method.as_str(), parts.uri.path()),
        method: parts.method.to_string(),
        path: parts.uri.path().to_string(),
        query: parts.uri.query().unwrap_or_default().to_string(),
        version: format!("{:?}", parts.version),
        remote,
        authority,
        secure,
        client,
        headers,
        body: Mutex::new(Some(body)),
        taken: Default::default(),
        limit: std::sync::atomic::AtomicUsize::new(limits.body),
        exceeded: Arc::default(),
        body_timeout: limits.body_timeout,
        stalled: Arc::default(),
        reply: Mutex::new(Some(reply)),
        upgrade: Mutex::new(upgrade),
        answering: Arc::default(),
    };
    // Dropped with this future when the client goes away first.
    let _abandoned = Abandoned(incoming.answering.clone());
    if sender.send(incoming).await.is_err() {
        return Ok(plain(503, "the server is stopping"));
    }
    Ok(answer.await.unwrap_or_else(|_| plain(500, "the handler ended without a response")))
}

/// The server's address, with the port it took.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_server_address(server: *mut NativeObject) -> *mut Str {
    str_new(native::get::<Server>(server).map_or("", |s| s.address.as_str()).as_bytes())
}

/// The next request, or the mark that the server stopped: after `stop`,
/// once its connections ended the requests they had, or its grace passed.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_next(server: *mut NativeObject) -> *mut NativeObject {
    let Some(server) = native::get::<Server>(server) else { return native::new(Request::closed()) };
    let (Some(requests), Some(life)) = (server.requests.clone(), server.life.clone()) else {
        return native::new(Request::closed());
    };
    let next = wait(async move {
        let (mut phase, mut open) = (life.phase.subscribe(), life.open.subscribe());
        let mut requests = requests.lock().await;
        loop {
            let now = *phase.borrow_and_update();
            if now == Phase::Closed {
                return None;
            }
            tokio::select! {
                request = requests.recv() => return request,
                _ = open.wait_for(|open| *open == 0), if now == Phase::Draining => return None,
                changed = phase.changed() => if changed.is_err() { return None },
            }
        }
    });
    native::new(next.flatten().unwrap_or_else(Request::closed))
}

/// Stops accepting connections and drains those open: each ends the
/// requests it has and closes, and after `grace` milliseconds those left
/// close as they are.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_server_stop(server: *mut NativeObject, grace: i64) {
    if let Some(life) = native::get::<Server>(server).and_then(|s| s.life.clone()) {
        let draining = life.phase.send_if_modified(|phase| {
            let serving = *phase == Phase::Serving;
            if serving {
                *phase = Phase::Draining;
            }
            serving
        });
        if draining {
            let mut listening = life.listening.subscribe();
            runtime().spawn(async move {
                let grace = std::time::Duration::from_millis(grace.max(0) as u64);
                let mut open = life.open.subscribe();
                let _ = tokio::time::timeout(grace, open.wait_for(|open| *open == 0)).await;
                life.phase.send_replace(Phase::Closed);
            });
            wait(async move { _ = listening.wait_for(|listening| !*listening).await });
        }
    }
    if let Some(path) = native::get::<Server>(server).and_then(|s| s.socket.as_ref()) {
        let _ = std::fs::remove_file(path);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_open(object: *mut NativeObject) -> u8 {
    native::get::<Request>(object).is_some_and(|r| r.open) as u8
}

/// `method\tpath\tquery\tversion\tremote`.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_head(object: *mut NativeObject) -> *mut Str {
    let r = request(object);
    let secure = r.secure as u8;
    let head = format!("{}\t{}\t{}\t{}\t{}\t{}\t{secure}", r.method, r.path, r.query, r.version, r.remote, r.authority);
    str_new(head.as_bytes())
}

/// The DER of the certificate the client showed, empty without one.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_certificate(object: *mut NativeObject) -> *mut ArrayHeader {
    byte_array(&request(object).client)
}

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_headers(object: *mut NativeObject) -> *mut ArrayHeader {
    string_array(&request(object).headers)
}

/// A request's body as the host of a stream.
struct RequestBody {
    body: Option<http_body_util::Limited<Idle>>,
    chunk: Bytes,
    exceeded: Arc<std::sync::atomic::AtomicBool>,
}

impl Host for RequestBody {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.chunk.is_empty() {
            let Some(body) = self.body.as_mut() else { return Ok(0) };
            match runtime().block_on(body.frame()) {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        self.chunk = data;
                    }
                }
                Some(Err(error)) => {
                    note_exceeded(error.as_ref(), &self.exceeded);
                    return Err(std::io::Error::other(error));
                }
                None => self.body = None,
            }
        }
        let count = buffer.len().min(self.chunk.len());
        buffer[..count].copy_from_slice(&self.chunk[..count]);
        self.chunk.advance(count);
        Ok(count)
    }
}

/// The task that answers the request, which a client that goes away before
/// the response cancels.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_answering(object: *mut NativeObject) {
    let request = request(object);
    let answering = &request.answering;
    answering.task.store(super::tasks::current_id(), std::sync::atomic::Ordering::SeqCst);
    if let Some(flight) = &request.flight {
        flight.answered_by(super::tasks::current_id());
    }
    super::debugger::serving(&request.method, &request.path);
    answering.cancel_if_gone();
}

/// The bytes the body may have, until it is read.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_limit(object: *mut NativeObject, bytes: i64) {
    request(object).limit.store(bytes.max(0) as usize, std::sync::atomic::Ordering::Release);
}

/// Whether a reading of the body went past its limit.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_exceeded(object: *mut NativeObject) -> u8 {
    request(object).exceeded.load(std::sync::atomic::Ordering::Acquire) as u8
}

/// Whether the body stopped arriving for longer than the server allows.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_stalled(object: *mut NativeObject) -> u8 {
    request(object).stalled.load(std::sync::atomic::Ordering::Acquire) as u8
}

/// The body as a stream of `std.io`; an empty one once taken.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_body(object: *mut NativeObject) -> *mut StreamObject {
    let r = request(object);
    let body = RequestBody { body: r.take_body(), chunk: Bytes::new(), exceeded: r.exceeded.clone() };
    crate::io::stream(Box::new(body), Mode::Long, false)
}

/// The whole body; on a failure, what was read and the failure recorded.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_read(object: *mut NativeObject) -> *mut ArrayHeader {
    let r = request(object);
    let Some(body) = r.take_body() else { return byte_array(&[]) };
    match wait(async move { body.collect().await }) {
        Some(Ok(collected)) => byte_array(&collected.to_bytes()),
        outcome => {
            let message = match outcome {
                Some(Err(error)) => r.body_failed(error.as_ref()),
                _ => "the task was cancelled".into(),
            };
            *lock(&r.failure) = Some(("body", message));
            byte_array(&[])
        }
    }
}

/// Responds with bytes.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_respond(
    object: *mut NativeObject,
    status: i64,
    headers: *mut ArrayHeader,
    data: *mut ArrayHeader,
) {
    request(object).send(response(status, &strings(headers), full(bytes_of(data))));
}

/// Responds with what a stream gives, as it gives it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_respond_stream(
    object: *mut NativeObject,
    status: i64,
    headers: *mut ArrayHeader,
    input: *mut StreamObject,
) {
    use futures_util::StreamExt;
    unsafe { super::tasks::tondo_rt_handle_retain(input as *mut u8) };
    let frames = stream_of(Source(input)).map(|chunk| chunk.map(Frame::data));
    request(object).send(response(status, &strings(headers), BodyExt::boxed_unsync(StreamBody::new(frames))));
}

/// The trailers of a reply, which its handler gives before the reply's
/// stream ends and which follow its last bytes, as gRPC's status does.
#[derive(Default)]
struct Trailers(Mutex<Vec<(String, String)>>);

#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_trailers() -> *mut NativeObject {
    native::new(Arc::new(Trailers::default()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_trailers_set(object: *mut NativeObject, name: *const Str, value: *const Str) {
    if let Some(trailers) = native::get::<Arc<Trailers>>(object) {
        let mut list = lock(&trailers.0);
        let name = text(name).to_ascii_lowercase();
        list.retain(|(n, _)| *n != name);
        list.push((name, text(value).to_string()));
    }
}

/// The names and values set, in order.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_trailers_list(object: *mut NativeObject) -> *mut ArrayHeader {
    let list = native::get::<Arc<Trailers>>(object)
        .map_or(Vec::new(), |t| lock(&t.0).iter().flat_map(|(name, value)| [name.clone(), value.clone()]).collect());
    string_array(&list)
}

/// Responds with what a stream gives, as it gives it, and then the
/// trailers the handler set by the time the stream ended.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_respond_trailed(
    object: *mut NativeObject,
    status: i64,
    headers: *mut ArrayHeader,
    input: *mut StreamObject,
    trailers: *mut NativeObject,
) {
    use futures_util::StreamExt;
    let Some(trailers) = native::get::<Arc<Trailers>>(trailers).cloned() else {
        return unsafe { tondo_rt_http_respond_stream(object, status, headers, input) };
    };
    unsafe { super::tasks::tondo_rt_handle_retain(input as *mut u8) };
    let frames = stream_of(Source(input)).map(|chunk| chunk.map(Frame::data));
    let last = futures_util::stream::once(async move {
        let mut map = hyper::HeaderMap::new();
        for (name, value) in lock(&trailers.0).iter() {
            if let (Ok(name), Ok(value)) = (
                hyper::header::HeaderName::try_from(name.as_str()),
                hyper::header::HeaderValue::try_from(value.as_str()),
            ) {
                map.append(name, value);
            }
        }
        Ok(Frame::trailers(map))
    });
    request(object).send(response(
        status,
        &strings(headers),
        BodyExt::boxed_unsync(StreamBody::new(frames.chain(last))),
    ));
}

/// `bytes=start-end`, `bytes=start-` or `bytes=-suffix` against a length:
/// the first and last byte, `None` for what is not one range, or `Err`
/// when it starts past the end.
fn range(value: &str, length: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(spec) = value.trim().strip_prefix("bytes=") else { return Ok(None) };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((start, end)) = spec.split_once('-') else { return Ok(None) };
    let (start, end) = (start.trim(), end.trim());
    let parsed = |text: &str| text.parse::<u64>().ok();
    let (first, last) = match (start.is_empty(), end.is_empty()) {
        (true, false) => {
            let Some(suffix) = parsed(end).filter(|&n| n > 0) else { return Err(()) };
            (length.saturating_sub(suffix), length.saturating_sub(1))
        }
        (false, _) => {
            let Some(first) = parsed(start) else { return Ok(None) };
            let last = if end.is_empty() { length.saturating_sub(1) } else { parsed(end).unwrap_or(u64::MAX) };
            (first, last.min(length.saturating_sub(1)))
        }
        _ => return Ok(None),
    };
    if length == 0 || first >= length || first > last {
        return Err(());
    }
    Ok(Some((first, last)))
}

/// Responds with a file: with a `Range`, the part asked for (206), so a
/// download can resume; with `If-None-Match` or `If-Modified-Since` that
/// still hold, 304; with `download`, as an attachment of that name. The
/// problem when there is no file, or an empty string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_respond_file(
    object: *mut NativeObject,
    headers: *mut ArrayHeader,
    path: *const Str,
    download: *const Str,
) -> *mut Str {
    let r = request(object);
    let (path, download, mut extra) = (text(path), text(download), strings(headers));
    let opened = {
        let path = path.clone();
        wait(async move {
            let (file, metadata) = blocking(move || -> std::io::Result<_> {
                let file = std::fs::File::open(&path)?;
                let metadata = file.metadata()?;
                if metadata.is_dir() {
                    return Err(std::io::Error::other("it is a directory"));
                }
                Ok((file, metadata))
            })
            .await??;
            Ok::<_, std::io::Error>((Disk(Some(file)), metadata))
        })
    };
    let (file, metadata) = match opened {
        Some(Ok(opened)) => opened,
        Some(Err(error)) => return str_new(error.to_string().as_bytes()),
        None => return str_new(b"the task was cancelled"),
    };
    let length = metadata.len();
    let modified = metadata.modified().ok();
    let seconds = modified.and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    if !download.is_empty() {
        let name = download.replace(['"', '\\'], "_");
        extra.extend(["content-disposition".into(), format!("attachment; filename=\"{name}\"")]);
    }
    let entity = Entity {
        length,
        etag: format!("\"{length:x}-{seconds:x}\""),
        modified: modified.map(|m| (seconds, httpdate::fmt_http_date(m))),
        mime: mime_guess::from_path(&path).first_or_octet_stream().to_string(),
    };
    respond_entity(r, extra, entity, |first, size| {
        let chunks = futures_util::stream::unfold((file, first, size), |(mut file, at, left)| async move {
            if left == 0 {
                return None;
            }
            let size = left.min(64 * 1024) as usize;
            let read = file
                .run(move |file| {
                    use std::io::Seek;
                    if at > 0 {
                        file.seek(std::io::SeekFrom::Start(at))?;
                    }
                    let mut buffer = vec![0; size];
                    let count = std::io::Read::read(file, &mut buffer)?;
                    buffer.truncate(count);
                    Ok(buffer)
                })
                .await;
            match read {
                Ok(buffer) if buffer.is_empty() => None,
                Ok(buffer) => {
                    let count = buffer.len() as u64;
                    Some((Ok(Frame::data(Bytes::from(buffer))), (file, 0, left - count)))
                }
                Err(error) => Some((Err(error), (file, 0, 0))),
            }
        });
        BodyExt::boxed_unsync(StreamBody::new(chunks))
    });
    str_new(b"")
}

/// A file the server reads or writes, each operation done where work that
/// blocks is (`http::blocking`).
struct Disk(Option<std::fs::File>);

impl Disk {
    async fn run<T: Send + 'static>(
        &mut self,
        op: impl FnOnce(&mut std::fs::File) -> std::io::Result<T> + Send + 'static,
    ) -> std::io::Result<T> {
        let mut file = self.0.take().ok_or_else(|| std::io::Error::other("the file failed before"))?;
        let (file, result) = blocking(move || {
            let result = op(&mut file);
            (file, result)
        })
        .await?;
        self.0 = Some(file);
        result
    }
}

/// Responds with an embedded file (standard library 14.27) as with a file
/// on disk: its content type from `name`, and the `ETag` of its `tag`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_respond_bytes(
    object: *mut NativeObject,
    headers: *mut ArrayHeader,
    name: *const Str,
    data: *mut ArrayHeader,
    tag: *const Str,
) {
    let (name, tag) = (text(name), text(tag));
    // The array's bytes in place: an embedded array is never changed.
    let data: &[u8] = match data.is_null() {
        true => &[],
        false => unsafe { std::slice::from_raw_parts(super::element(data, 1, 0), (*data).len) },
    };
    let entity = Entity {
        length: data.len() as u64,
        etag: format!("\"{tag}\""),
        modified: None,
        mime: mime_guess::from_path(&name).first_or_octet_stream().to_string(),
    };
    respond_entity(request(object), strings(headers), entity, |first, size| {
        full(Bytes::copy_from_slice(&data[first as usize..(first + size) as usize]))
    });
}

/// What a file answers with: its length, its `ETag`, when it changed, in
/// seconds and as HTTP writes it, and its content type.
struct Entity {
    length: u64,
    etag: String,
    modified: Option<(u64, String)>,
    mime: String,
}

/// Responds with an entity as its request's conditions ask: 304 when it did
/// not change, 416 for a range outside it, or the range asked for (206) or
/// all of it, whose bytes `body` gives from a first byte and a size.
fn respond_entity(r: &Request, mut extra: Vec<String>, entity: Entity, body: impl FnOnce(u64, u64) -> Body) {
    let [none_match, modified_since, asked, if_range] =
        ["if-none-match", "if-modified-since", "range", "if-range"].map(|name| r.header(name).map(String::from));
    let Entity { length, etag, modified, mime } = entity;
    let unchanged = match (none_match, modified_since, &modified) {
        (Some(tags), _, _) => tags.split(',').any(|tag| tag.trim() == etag || tag.trim() == "*"),
        (None, Some(since), Some((seconds, _))) => httpdate::parse_http_date(&since)
            .ok()
            .and_then(|since| since.duration_since(std::time::UNIX_EPOCH).ok())
            .is_some_and(|since| since.as_secs() >= *seconds),
        _ => false,
    };
    let last_modified = modified.map(|(_, text)| text).unwrap_or_default();
    extra.extend(["accept-ranges".into(), "bytes".into(), "etag".into(), etag.clone()]);
    if !last_modified.is_empty() {
        extra.extend(["last-modified".into(), last_modified.clone()]);
    }
    if unchanged {
        r.send(response(304, &extra, full(Bytes::new())));
        return;
    }
    // A range holds only while the file is the one `If-Range` names.
    let asked = asked.filter(|_| if_range.as_ref().is_none_or(|tag| *tag == etag || *tag == last_modified));
    let (status, first, last) = match asked.map(|value| range(&value, length)) {
        Some(Ok(Some((first, last)))) => (206, first, last),
        Some(Err(())) => {
            extra.extend(["content-range".into(), format!("bytes */{length}")]);
            r.send(response(416, &extra, full(Bytes::new())));
            return;
        }
        _ => (200, 0, length.saturating_sub(1)),
    };
    let size = if length == 0 { 0 } else { last - first + 1 };
    extra.extend(["content-type".into(), mime, "content-length".into(), size.to_string()]);
    if status == 206 {
        extra.extend(["content-range".into(), format!("bytes {first}-{last}/{length}")]);
    }
    if r.method == "HEAD" || size == 0 {
        r.send(response(status, &extra, full(Bytes::new())));
        return;
    }
    r.send(response(status, &extra, body(first, size)));
}

/// Writes the body to a file as it arrives, from `offset` on, which keeps
/// the bytes before it: an upload that resumes. How many bytes the file
/// has after, or -1 with the failure recorded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_save_body(object: *mut NativeObject, path: *const Str, offset: i64) -> i64 {
    let r = request(object);
    let (path, body, exceeded) = (text(path), r.take_body(), r.exceeded.clone());
    let saved = wait(async move {
        let offset = u64::try_from(offset).unwrap_or(0);
        let opened = blocking(move || -> std::io::Result<_> {
            use std::io::Seek;
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::OpenOptions::new().create(true).write(true).truncate(offset == 0).open(&path)?;
            let length = file.metadata()?.len();
            if offset > length {
                return Err(std::io::Error::other(format!(
                    "the file has {length} bytes, fewer than the offset {offset}"
                )));
            }
            file.set_len(offset)?;
            file.seek(std::io::SeekFrom::Start(offset))?;
            Ok(file)
        })
        .await??;
        let mut file = Disk(Some(opened));
        if let Some(mut body) = body {
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|error| {
                    note_exceeded(error.as_ref(), &exceeded);
                    std::io::Error::other(error)
                })?;
                if let Ok(data) = frame.into_data() {
                    file.run(move |file| std::io::Write::write_all(file, &data)).await?;
                }
            }
        }
        file.run(|file| Ok(file.metadata()?.len())).await
    });
    match saved {
        Some(Ok(length)) => length as i64,
        outcome => {
            let message = match outcome {
                Some(Err(error)) => error.to_string(),
                _ => "the task was cancelled".into(),
            };
            *lock(&r.failure) = Some(("body", message));
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// Multipart uploads
// ---------------------------------------------------------------------------

struct Form {
    failure: Failure,
    reader: Option<Arc<tokio::sync::Mutex<multer::Multipart<'static>>>>,
}

/// One part of a multipart body: its name, file name and content type, and
/// its bytes until read.
struct Upload {
    failure: Failure,
    done: bool,
    name: String,
    file_name: String,
    content_type: String,
    field: Mutex<Option<multer::Field<'static>>>,
}

/// Whether the body was taken to be read already.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_taken(object: *mut NativeObject) -> u8 {
    request(object).taken.load(std::sync::atomic::Ordering::Acquire) as u8
}

/// The parts of a `multipart/form-data` body that a middleware read and
/// gave on, from its bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_request_form_of(
    object: *mut NativeObject,
    data: *mut ArrayHeader,
) -> *mut NativeObject {
    let r = request(object);
    let data = Bytes::from(bytes_of(data));
    match r.header("content-type").map(multer::parse_boundary) {
        Some(Ok(boundary)) => {
            let stream = futures_util::stream::once(async move { Ok::<Bytes, std::convert::Infallible>(data) });
            let reader = multer::Multipart::new(stream, boundary);
            native::new(Form { failure: Mutex::new(None), reader: Some(Arc::new(tokio::sync::Mutex::new(reader))) })
        }
        _ => native::new(Form {
            failure: Mutex::new(Some(("body", "it is not multipart/form-data".into()))),
            reader: None,
        }),
    }
}

/// The parts of a `multipart/form-data` body, read as they arrive.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_request_form(object: *mut NativeObject) -> *mut NativeObject {
    let r = request(object);
    let boundary = r.header("content-type").map(multer::parse_boundary);
    let body = r.take_body();
    match (boundary, body) {
        (Some(Ok(boundary)), Some(body)) => {
            let exceeded = r.exceeded.clone();
            let body = body.map_err(move |error| {
                note_exceeded(error.as_ref(), &exceeded);
                error
            });
            let stream = http_body_util::BodyDataStream::new(body);
            let reader = multer::Multipart::new(stream, boundary);
            native::new(Form { failure: Mutex::new(None), reader: Some(Arc::new(tokio::sync::Mutex::new(reader))) })
        }
        (Some(Ok(_)), None) => {
            native::new(Form { failure: Mutex::new(Some(("body", "the body was already read".into()))), reader: None })
        }
        _ => native::new(Form {
            failure: Mutex::new(Some(("body", "it is not multipart/form-data".into()))),
            reader: None,
        }),
    }
}

fn upload(failure: Option<(&'static str, String)>, field: Option<multer::Field<'static>>) -> *mut NativeObject {
    let describe = |f: &multer::Field<'static>| {
        (
            f.name().unwrap_or_default().to_string(),
            f.file_name().unwrap_or_default().to_string(),
            f.content_type().map(|m| m.to_string()).unwrap_or_default(),
        )
    };
    let (name, file_name, content_type) = field.as_ref().map(describe).unwrap_or_default();
    native::new(Upload {
        failure: Mutex::new(failure),
        done: field.is_none(),
        name,
        file_name,
        content_type,
        field: Mutex::new(field),
    })
}

/// The next part, or the end.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_form_next(form: *mut NativeObject) -> *mut NativeObject {
    let Some(reader) = native::get::<Form>(form).and_then(|f| f.reader.clone()) else { return upload(None, None) };
    match wait(async move { reader.lock().await.next_field().await }) {
        Some(Ok(field)) => upload(None, field),
        Some(Err(error)) => upload(Some(("body", error.to_string())), None),
        None => upload(Some(("protocol", "the task was cancelled".into())), None),
    }
}

/// `name\tfileName\tcontentType`, or an empty string at the end.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_upload_head(object: *mut NativeObject) -> *mut Str {
    match native::get::<Upload>(object) {
        Some(u) if !u.done => str_new(format!("{}\t{}\t{}", u.name, u.file_name, u.content_type).as_bytes()),
        _ => str_new(b""),
    }
}

/// A part's bytes; on a failure, the failure recorded.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_upload_read(object: *mut NativeObject) -> *mut ArrayHeader {
    let Some(u) = native::get::<Upload>(object) else { return byte_array(&[]) };
    let Some(field) = lock(&u.field).take() else { return byte_array(&[]) };
    match wait(async move { field.bytes().await }) {
        Some(Ok(bytes)) => byte_array(&bytes),
        outcome => {
            let message = match outcome {
                Some(Err(error)) => error.to_string(),
                _ => "the task was cancelled".into(),
            };
            *lock(&u.failure) = Some(("body", message));
            byte_array(&[])
        }
    }
}

/// Writes a part to a file as it arrives; how many bytes, or -1 with the
/// failure recorded.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tondo_rt_http_upload_save(object: *mut NativeObject, path: *const Str) -> i64 {
    let Some(u) = native::get::<Upload>(object) else { return -1 };
    let (path, field) = (text(path), lock(&u.field).take());
    let saved = wait(async move {
        let created = blocking(move || -> std::io::Result<_> {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::File::create(&path)
        })
        .await??;
        let (mut file, mut total) = (Disk(Some(created)), 0u64);
        if let Some(mut field) = field {
            while let Some(chunk) = field.chunk().await.map_err(std::io::Error::other)? {
                total += chunk.len() as u64;
                file.run(move |file| std::io::Write::write_all(file, &chunk)).await?;
            }
        }
        Ok::<u64, std::io::Error>(total)
    });
    match saved {
        Some(Ok(total)) => total as i64,
        outcome => {
            let message = match outcome {
                Some(Err(error)) => error.to_string(),
                _ => "the task was cancelled".into(),
            };
            *lock(&u.failure) = Some(("body", message));
            -1
        }
    }
}

// ---------------------------------------------------------------------------
// WebSockets and event streams
// ---------------------------------------------------------------------------

/// Accepts a WebSocket the request asks for: answers 101 and gives the
/// socket, or one that failed when the request is not a WebSocket's.
#[unsafe(no_mangle)]
pub extern "C" fn tondo_rt_http_accept_websocket(object: *mut NativeObject) -> *mut NativeObject {
    use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
    use tokio_tungstenite::tungstenite::protocol::Role;
    let r = request(object);
    let failed = |message: &str| {
        let socket = super::http::Socket::failed("protocol", message.to_string());
        native::new(socket)
    };
    let (Some(upgrade), Some(key)) = (lock(&r.upgrade).take(), r.header("sec-websocket-key")) else {
        r.send(plain(400, "a WebSocket handshake was expected"));
        return failed("the request does not open a WebSocket");
    };
    let accept = derive_accept_key(key.as_bytes());
    r.send(response(
        101,
        &[
            "upgrade".into(),
            "websocket".into(),
            "connection".into(),
            "Upgrade".into(),
            "sec-websocket-accept".into(),
            accept,
        ],
        full(Bytes::new()),
    ));
    let upgraded = wait(async move {
        let upgraded = upgrade.await?;
        let io = hyper_util::rt::TokioIo::new(upgraded);
        Ok::<_, hyper::Error>(tokio_tungstenite::WebSocketStream::from_raw_socket(io, Role::Server, None).await)
    });
    match upgraded {
        Some(Ok(connection)) => native::new(socket(connection)),
        Some(Err(error)) => failed(&error.to_string()),
        None => failed("the task was cancelled"),
    }
}
