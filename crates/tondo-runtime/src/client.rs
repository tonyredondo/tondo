//! The client of `std.http` (docs/speed.md, H2, step 9): hyper's own
//! connections on the workers' poller, which the task that sends polls in
//! place, with the futures of the connections themselves at the workers'
//! turns, instead of reqwest on tokio's threads. What reqwest gave it does
//! too: a pool of connections by origin, HTTP/2 when TLS's ALPN gives it,
//! the environment's proxies, redirects, cookies, the decompression of
//! gzip, deflate and brotli, and multipart forms.

use bytes::Bytes;
use http_body_util::BodyExt;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A body as hyper sends it.
pub(crate) type Outgoing = http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>;

/// A connection either way, as TLS and hyper read and write it.
trait Duplex: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send + Unpin + 'static> Duplex for T {}

/// Why a request has no response: what kind of failure, as `HttpError`
/// tells them apart, and what it says.
pub(crate) type Failure = (&'static str, String);

fn failed(kind: &'static str, error: impl std::fmt::Display) -> Failure {
    (kind, error.to_string())
}

/// An error with the errors that caused it, as one message.
fn with_causes(error: &(dyn std::error::Error + 'static)) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !message.contains(&cause_text) {
            message.push_str(": ");
            message.push_str(&cause_text);
        }
        source = cause.source();
    }
    message
}

/// Where a connection goes: the origin it serves, and the proxy it goes
/// through, or the Unix socket.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Route {
    secure: bool,
    host: String,
    port: u16,
    via: Via,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Via {
    Direct,
    /// A proxy that is sent the whole URL, for a plain origin.
    Forward(Proxy),
    /// A proxy that a `CONNECT` turns into a tunnel to a TLS origin.
    Tunnel(Proxy),
    Socket(std::path::PathBuf),
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Proxy {
    secure: bool,
    host: String,
    port: u16,
    /// `Proxy-Authorization`, from the proxy's URL.
    authorization: Option<String>,
}

/// The proxies of the environment: `HTTP_PROXY`, `HTTPS_PROXY`,
/// `ALL_PROXY` and `NO_PROXY`, each in lower case first, as curl reads them.
#[derive(Default)]
struct Proxies {
    http: Option<Proxy>,
    https: Option<Proxy>,
    bypass: Vec<String>,
}

impl Proxies {
    fn from_environment() -> Proxies {
        let read = |upper: &str| {
            let names = [upper.to_lowercase(), upper.to_string()];
            names.into_iter().find_map(|name| std::env::var(name).ok()).filter(|value| !value.is_empty())
        };
        let all = read("ALL_PROXY");
        let proxy = |value: Option<String>| value.or_else(|| all.clone()).and_then(|url| Proxy::parse(&url));
        let bypass = read("NO_PROXY")
            .map(|list| {
                list.split(',').map(|entry| entry.trim().to_ascii_lowercase()).filter(|e| !e.is_empty()).collect()
            })
            .unwrap_or_default();
        Proxies { http: proxy(read("HTTP_PROXY")), https: proxy(read("HTTPS_PROXY")), bypass }
    }

    /// The proxy for a URL, unless `NO_PROXY` names its host: the host
    /// itself, a domain it ends in, or `*`.
    fn of(&self, secure: bool, host: &str) -> Option<&Proxy> {
        let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let bypassed = self.bypass.iter().any(|entry| {
            let entry = entry.trim_start_matches('.');
            entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
        });
        match (bypassed, secure) {
            (true, _) => None,
            (false, true) => self.https.as_ref(),
            (false, false) => self.http.as_ref(),
        }
    }
}

impl Proxy {
    fn parse(text: &str) -> Option<Proxy> {
        // A proxy written as `host:port` is one of HTTP.
        let text = if text.contains("://") { text.to_string() } else { format!("http://{text}") };
        let url = url::Url::parse(&text).ok()?;
        let secure = match url.scheme() {
            "http" => false,
            "https" => true,
            _ => return None,
        };
        let authorization = (!url.username().is_empty()).then(|| {
            let user = percent_decoded(url.username());
            let password = percent_decoded(url.password().unwrap_or(""));
            let encoded = crate::codec::base64_encode(format!("{user}:{password}").as_bytes(), false);
            format!("Basic {}", String::from_utf8_lossy(&encoded))
        });
        Some(Proxy { secure, host: url.host_str()?.to_string(), port: url.port_or_known_default()?, authorization })
    }
}

fn percent_decoded(text: &str) -> String {
    url::form_urlencoded::parse(text.as_bytes()).map(|(key, _)| key.into_owned()).collect()
}

/// A connection: its sending half, as hyper gives it for each version, and
/// its own future, which runs at the workers' turns and which a task that
/// waits on the connection polls in place too, as the server's do.
struct Sender {
    half: Half,
    connection: Arc<crate::server::Turning>,
}

enum Half {
    One(hyper::client::conn::http1::SendRequest<Outgoing>),
    Two(hyper::client::conn::http2::SendRequest<Outgoing>),
}

impl Sender {
    fn usable(&self) -> bool {
        match &self.half {
            Half::One(sender) => sender.is_ready(),
            Half::Two(sender) => !sender.is_closed(),
        }
    }

    fn closed(&self) -> bool {
        match &self.half {
            Half::One(sender) => sender.is_closed(),
            Half::Two(sender) => sender.is_closed(),
        }
    }

    /// Another sender of a connection of HTTP/2, which takes many requests.
    fn shared(&self) -> Option<Sender> {
        match &self.half {
            Half::Two(sender) => Some(Sender { half: Half::Two(sender.clone()), connection: self.connection.clone() }),
            Half::One(_) => None,
        }
    }
}

/// Polls `future`, and polls in place the connection it waits on first:
/// what the connection read is there at once, not at a worker's next turn.
async fn driven<F: std::future::Future>(connection: &Arc<crate::server::Turning>, future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|context| {
        connection.run();
        future.as_mut().poll(context)
    })
    .await
}

/// What a client keeps: its TLS, how it connects and how far it follows,
/// its cookies, and the connections its requests let go, by route.
pub(crate) struct Client {
    tls: tokio_rustls::TlsConnector,
    connect_timeout: Option<Duration>,
    redirects: usize,
    socket: Option<std::path::PathBuf>,
    cookies: Option<Mutex<cookie_store::CookieStore>>,
    proxies: Proxies,
    idle: Mutex<HashMap<Route, Vec<Sender>>>,
    /// While a secure route is not known to give HTTP/1, one request
    /// connects and the others wait to share its connection, as HTTP/2's
    /// takes them all; one of HTTP/1 connects as many as want one.
    connecting: Mutex<HashMap<Route, Arc<tokio::sync::Mutex<()>>>>,
    plain: Mutex<std::collections::HashSet<Route>>,
}

/// The connections of one route a client keeps, at most: more than that
/// go when their requests end.
const IDLE: usize = 64;

impl Client {
    pub(crate) fn new(
        tls: rustls::ClientConfig,
        connect_timeout: Option<Duration>,
        redirects: usize,
        socket: Option<std::path::PathBuf>,
        cookies: bool,
    ) -> Client {
        Client {
            tls: tokio_rustls::TlsConnector::from(Arc::new(tls)),
            connect_timeout,
            redirects,
            socket,
            cookies: cookies.then(|| Mutex::new(cookie_store::CookieStore::new())),
            proxies: Proxies::from_environment(),
            idle: Mutex::default(),
            connecting: Mutex::default(),
            plain: Mutex::default(),
        }
    }
}

/// Runs the futures of hyper's connections at the workers' turns, each until
/// its connection ends.
#[derive(Clone)]
struct Detached;

impl<F: std::future::Future<Output = ()> + Send + 'static> hyper::rt::Executor<F> for Detached {
    fn execute(&self, future: F) {
        let _ = crate::server::detach(future);
    }
}

impl Client {
    /// A connection for `route`: one it kept that can take a request now,
    /// or a new one.
    async fn sender(&self, route: &Route) -> Result<Sender, Failure> {
        if let Some(kept) = self.kept(route) {
            return Ok(kept);
        }
        let alone = route.secure && !self.plain.lock().unwrap_or_else(|e| e.into_inner()).contains(route);
        let turn = alone.then(|| {
            let mut connecting = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
            connecting.entry(route.clone()).or_default().clone()
        });
        let _turn = match &turn {
            Some(turn) => {
                let turn = turn.lock().await;
                // Another made one meanwhile, which may take this request too.
                if let Some(kept) = self.kept(route) {
                    return Ok(kept);
                }
                Some(turn)
            }
            None => None,
        };
        let connecting = self.connect(route);
        let sender = match self.connect_timeout {
            Some(limit) => tokio::time::timeout(limit, connecting)
                .await
                .map_err(|_| failed("timeout", "the connection timed out"))??,
            None => connecting.await?,
        };
        match sender.shared() {
            Some(shared) => self.keep(route, shared),
            None => _ = self.plain.lock().unwrap_or_else(|e| e.into_inner()).insert(route.clone()),
        }
        Ok(sender)
    }

    /// A connection of `route` the client kept that can take a request now.
    fn kept(&self, route: &Route) -> Option<Sender> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let kept = idle.get_mut(route)?;
        kept.retain(|sender| !sender.closed());
        let at = kept.iter().position(Sender::usable)?;
        // HTTP/2 takes many requests at once: it stays.
        Some(kept[at].shared().unwrap_or_else(|| kept.swap_remove(at)))
    }

    /// Keeps a connection of HTTP/1 its request let go, for the next.
    fn keep(&self, route: &Route, sender: Sender) {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let kept = idle.entry(route.clone()).or_default();
        if kept.len() < IDLE {
            kept.push(sender);
        }
    }

    /// A new connection for `route`: through its socket, its proxy or to
    /// its host, in TLS for a secure origin, in HTTP/2 when ALPN gives it.
    async fn connect(&self, route: &Route) -> Result<Sender, Failure> {
        let io: Box<dyn Duplex> = match &route.via {
            Via::Socket(path) => Box::new(unix(path).await?),
            Via::Direct => tcp(&route.host, route.port).await?,
            Via::Forward(proxy) => self.to_proxy(proxy).await?,
            Via::Tunnel(proxy) => {
                let through = self.to_proxy(proxy).await?;
                tunnel(through, proxy, &route.host, route.port).await?
            }
        };
        if !route.secure || matches!(route.via, Via::Socket(_)) {
            return one(io).await;
        }
        let name = rustls_pki_types::ServerName::try_from(route.host.trim_start_matches('[').trim_end_matches(']'))
            .map_err(|error| failed("url", error))?
            .to_owned();
        let secured = self.tls.connect(name, io).await.map_err(|error| failed("connect", with_causes(&error)))?;
        let alpn = secured.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        match alpn.as_deref() {
            Some(b"h2") => two(Box::new(secured)).await,
            _ => one(Box::new(secured)).await,
        }
    }

    async fn to_proxy(&self, proxy: &Proxy) -> Result<Box<dyn Duplex>, Failure> {
        let io = tcp(&proxy.host, proxy.port).await?;
        if !proxy.secure {
            return Ok(io);
        }
        let name = rustls_pki_types::ServerName::try_from(proxy.host.clone()).map_err(|error| failed("url", error))?;
        let secured = self.tls.connect(name, io).await.map_err(|error| failed("connect", with_causes(&error)))?;
        Ok(Box::new(secured))
    }
}

/// A TCP connection to the first address of `host` that takes one, on the
/// workers' poller.
async fn tcp(host: &str, port: u16) -> Result<Box<dyn Duplex>, Failure> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let addresses = crate::net::addresses(bare, port).await.map_err(|error| failed("connect", error))?;
    let mut last = failed("connect", format!("{host} has no address"));
    for address in addresses {
        match crate::net::connect(address).await {
            Ok(socket) => {
                let _ = socket.io().set_nodelay(true);
                return Ok(Box::new(crate::net::Polled::from(socket)));
            }
            Err(error) => last = failed("connect", format!("{address}: {error}")),
        }
    }
    Err(last)
}

/// A connection to a Unix socket, which connects at once or not at all.
async fn unix(path: &std::path::Path) -> Result<crate::net::Polled<mio::net::UnixStream>, Failure> {
    let connect = || -> std::io::Result<_> {
        let socket = std::os::unix::net::UnixStream::connect(path)?;
        socket.set_nonblocking(true)?;
        crate::net::Polled::new(mio::net::UnixStream::from_std(socket))
    };
    connect().map_err(|error| failed("connect", format!("{}: {error}", path.display())))
}

/// A tunnel through a proxy to `host`, which `CONNECT` opens.
async fn tunnel(through: Box<dyn Duplex>, proxy: &Proxy, host: &str, port: u16) -> Result<Box<dyn Duplex>, Failure> {
    let (mut sender, connection) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(through))
        .await
        .map_err(|error| failed("connect", with_causes(&error)))?;
    let _ = crate::server::detach(async move {
        let _ = connection.with_upgrades().await;
    });
    let authority = format!("{host}:{port}");
    let mut request =
        hyper::Request::builder().method(hyper::Method::CONNECT).uri(&authority).header("host", &authority);
    if let Some(authorization) = &proxy.authorization {
        request = request.header("proxy-authorization", authorization);
    }
    let request = request.body(empty()).map_err(|error| failed("url", error))?;
    let response = sender.send_request(request).await.map_err(|error| failed("connect", with_causes(&error)))?;
    if !response.status().is_success() {
        return Err(failed("connect", format!("the proxy answered CONNECT with {}", response.status())));
    }
    let upgraded = hyper::upgrade::on(response).await.map_err(|error| failed("connect", with_causes(&error)))?;
    Ok(Box::new(hyper_util::rt::TokioIo::new(upgraded)))
}

async fn one(io: Box<dyn Duplex>) -> Result<Sender, Failure> {
    let (sender, connection) = hyper::client::conn::http1::handshake(hyper_util::rt::TokioIo::new(io))
        .await
        .map_err(|error| failed("connect", with_causes(&error)))?;
    let connection = crate::server::detach(async move {
        let _ = connection.await;
    });
    Ok(Sender { half: Half::One(sender), connection })
}

async fn two(io: Box<dyn Duplex>) -> Result<Sender, Failure> {
    let (sender, connection) = hyper::client::conn::http2::handshake(Detached, hyper_util::rt::TokioIo::new(io))
        .await
        .map_err(|error| failed("connect", with_causes(&error)))?;
    let connection = crate::server::detach(async move {
        let _ = connection.await;
    });
    Ok(Sender { half: Half::Two(sender), connection })
}

pub(crate) fn empty() -> Outgoing {
    http_body_util::Empty::new().map_err(|never| match never {}).boxed_unsync()
}

/// What a request sends: its method, URL, headers and body.
pub(crate) struct Request {
    pub(crate) method: hyper::Method,
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Body,
    pub(crate) timeout: Option<Duration>,
}

/// A request's body: bytes, which a redirect can send again, or a stream or
/// a form, which it cannot.
pub(crate) enum Body {
    Empty,
    Bytes(Bytes, String),
    Stream(Outgoing, Option<u64>, String),
    Form(Form),
}

/// A response's head, and its body as it comes, decoded.
pub(crate) struct Answer {
    pub(crate) status: hyper::StatusCode,
    pub(crate) version: hyper::Version,
    pub(crate) url: String,
    pub(crate) headers: hyper::HeaderMap,
    pub(crate) body: Received,
}

/// The statuses that redirect, and whether the next request keeps the
/// method and the body: 307 and 308 do; 301, 302 and 303 make it a GET,
/// as browsers and reqwest do, but for a HEAD.
fn redirects(status: hyper::StatusCode) -> Option<bool> {
    match status.as_u16() {
        301..=303 => Some(false),
        307 | 308 => Some(true),
        _ => None,
    }
}

impl Client {
    /// Sends a request and follows its redirects: the response's head,
    /// once it comes, or why there is none.
    pub(crate) async fn send(&self, request: Request) -> Result<Answer, Failure> {
        let deadline = request.timeout.map(|limit| Instant::now() + limit);
        let sending = self.follow(request, deadline);
        match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline.into(), sending)
                .await
                .map_err(|_| failed("timeout", "the request timed out"))?,
            None => sending.await,
        }
    }

    async fn follow(&self, request: Request, deadline: Option<Instant>) -> Result<Answer, Failure> {
        let Request { mut method, url, mut headers, mut body, .. } = request;
        let mut url = url::Url::parse(&url).map_err(|error| failed("url", format!("{url}: {error}")))?;
        let mut hops = 0;
        loop {
            // A body sent once is gone: bytes go again, a stream does not.
            let again = match &body {
                Body::Empty => Some(Body::Empty),
                Body::Bytes(data, kind) => Some(Body::Bytes(data.clone(), kind.clone())),
                _ => None,
            };
            let answer = self.exchange(&method, &url, &headers, body, deadline).await?;
            let location = answer.headers.get(hyper::header::LOCATION).and_then(|value| value.to_str().ok());
            let (Some(keeps), Some(location)) = (redirects(answer.status), location) else { return Ok(answer) };
            if self.redirects == 0 {
                return Ok(answer);
            }
            if hops == self.redirects {
                return Err(failed("protocol", format!("{url}: more than {} redirects", self.redirects)));
            }
            hops += 1;
            let next = url.join(location).map_err(|error| failed("url", format!("{location}: {error}")))?;
            // What tells the server who asks goes only to the same origin.
            if next.origin() != url.origin() {
                headers.retain(|(name, _)| {
                    !["authorization", "cookie", "proxy-authorization"].contains(&name.to_ascii_lowercase().as_str())
                });
            }
            body = match (keeps, again) {
                (true, Some(again)) => again,
                (true, None) => return Ok(answer),
                (false, _) => {
                    if method != hyper::Method::HEAD {
                        method = hyper::Method::GET;
                    }
                    headers.retain(|(name, _)| {
                        !["content-type", "content-length"].contains(&name.to_ascii_lowercase().as_str())
                    });
                    Body::Empty
                }
            };
            url = next;
        }
    }

    /// One request to one URL: its connection, its head and its body; the
    /// response's head, with the cookies it sets kept.
    async fn exchange(
        &self,
        method: &hyper::Method,
        url: &url::Url,
        headers: &[(String, String)],
        body: Body,
        deadline: Option<Instant>,
    ) -> Result<Answer, Failure> {
        let secure = match url.scheme() {
            "http" => false,
            "https" => true,
            other => return Err(failed("url", format!("{url}: `{other}` is not http or https"))),
        };
        let host = url.host_str().ok_or_else(|| failed("url", format!("{url}: no host")))?.to_string();
        let port = url.port_or_known_default().unwrap_or(if secure { 443 } else { 80 });
        let via = match (&self.socket, self.proxies.of(secure, &host)) {
            (Some(path), _) => Via::Socket(path.clone()),
            (None, Some(proxy)) if secure => Via::Tunnel(proxy.clone()),
            (None, Some(proxy)) => Via::Forward(proxy.clone()),
            (None, None) => Via::Direct,
        };
        let route = Route { secure, host: host.clone(), port, via };
        let mut sender = self.sender(&route).await?;
        let two = matches!(sender.half, Half::Two(_));
        // HTTP/2 names the origin in the URI; HTTP/1 in `Host`, and a
        // proxy that forwards takes the whole URL.
        let target = match (&route.via, two) {
            (Via::Forward(_), _) | (_, true) => url.as_str().to_string(),
            _ => match url.query() {
                Some(query) => format!("{}?{query}", url.path()),
                None => url.path().to_string(),
            },
        };
        let mut builder = hyper::Request::builder().method(method.clone()).uri(target);
        let has = |name: &str| headers.iter().any(|(given, _)| given.eq_ignore_ascii_case(name));
        if !two {
            let authority = match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.clone(),
            };
            builder = builder.header(hyper::header::HOST, authority);
        }
        if let Via::Forward(Proxy { authorization: Some(authorization), .. }) = &route.via {
            builder = builder.header("proxy-authorization", authorization);
        }
        if !has("user-agent") {
            builder = builder.header(hyper::header::USER_AGENT, concat!("tondo/", env!("CARGO_PKG_VERSION")));
        }
        // What this client decodes, unless the program asks for another.
        let decodes = !has("accept-encoding");
        if decodes {
            builder = builder.header(hyper::header::ACCEPT_ENCODING, "gzip, deflate, br");
        }
        if let Some(cookies) = &self.cookies
            && !has("cookie")
        {
            let jar = cookies.lock().unwrap_or_else(|e| e.into_inner());
            let pairs: Vec<String> =
                jar.get_request_values(url).map(|(name, value)| format!("{name}={value}")).collect();
            if !pairs.is_empty() {
                builder = builder.header(hyper::header::COOKIE, pairs.join("; "));
            }
        }
        for (name, value) in headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        let (outgoing, length, kind) = match body {
            Body::Empty => (empty(), None, None),
            Body::Bytes(data, kind) => {
                let length = data.len() as u64;
                (
                    http_body_util::Full::new(data).map_err(|never| match never {}).boxed_unsync(),
                    Some(length),
                    Some(kind),
                )
            }
            Body::Stream(stream, length, kind) => (stream, length, Some(kind)),
            Body::Form(form) => {
                let (stream, length, kind) = form.into_body();
                (stream, length, Some(kind))
            }
        };
        if let Some(kind) = kind.filter(|kind| !kind.is_empty() && !has("content-type")) {
            builder = builder.header(hyper::header::CONTENT_TYPE, kind);
        }
        if let Some(length) = length.filter(|_| !has("content-length")) {
            builder = builder.header(hyper::header::CONTENT_LENGTH, length);
        }
        let request = builder.body(outgoing).map_err(|error| failed("url", with_causes(&error)))?;
        let connection = sender.connection.clone();
        let sent = match &mut sender.half {
            Half::One(one) => driven(&connection, one.send_request(request)).await,
            Half::Two(two) => driven(&connection, two.send_request(request)).await,
        };
        let response = sent.map_err(|error| failed("protocol", format!("{method} {url}: {}", with_causes(&error))))?;
        if let Half::One(_) = sender.half {
            // Ready again once the program read the body.
            self.keep(&route, sender);
        }
        if let Some(cookies) = &self.cookies {
            let set = response.headers().get_all(hyper::header::SET_COOKIE).iter();
            let parsed = set
                .filter_map(|value| value.to_str().ok())
                .filter_map(|text| cookie::Cookie::parse(text.to_owned()).ok());
            cookies.lock().unwrap_or_else(|e| e.into_inner()).store_response_cookies(parsed, url);
        }
        let (mut parts, incoming) = response.into_parts();
        let encoding = parts.headers.get(hyper::header::CONTENT_ENCODING).and_then(|value| value.to_str().ok());
        let decoder = encoding.filter(|_| decodes).and_then(Decoder::of);
        if decoder.is_some() {
            // The program reads what was decoded, which has other lengths.
            parts.headers.remove(hyper::header::CONTENT_ENCODING);
            parts.headers.remove(hyper::header::CONTENT_LENGTH);
        }
        Ok(Answer {
            status: parts.status,
            version: parts.version,
            url: url.to_string(),
            headers: parts.headers,
            body: Received { incoming: Some(incoming), decoder, deadline, connection },
        })
    }
}

/// A response's body as it comes: its chunks, decoded when the server
/// encoded them for this client, until the request's time is up.
pub(crate) struct Received {
    incoming: Option<hyper::body::Incoming>,
    decoder: Option<Decoder>,
    deadline: Option<Instant>,
    connection: Arc<crate::server::Turning>,
}

impl Received {
    /// The next chunk, or none once it ended.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>, Failure> {
        loop {
            let Some(incoming) = self.incoming.as_mut() else { return Ok(None) };
            let next = driven(&self.connection, incoming.frame());
            let frame = match self.deadline {
                Some(deadline) => tokio::time::timeout_at(deadline.into(), next)
                    .await
                    .map_err(|_| failed("timeout", "the request timed out"))?,
                None => next.await,
            };
            let data = match frame {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => data,
                    // Trailers.
                    Err(_) => continue,
                },
                Some(Err(error)) => return Err(failed("body", with_causes(&error))),
                None => {
                    self.incoming = None;
                    return match self.decoder.take() {
                        Some(decoder) => decoder.finish().map(|rest| Some(rest).filter(|rest| !rest.is_empty())),
                        None => Ok(None),
                    };
                }
            };
            match &mut self.decoder {
                None => return Ok(Some(data)),
                Some(decoder) => {
                    let decoded = decoder.feed(&data)?;
                    if !decoded.is_empty() {
                        return Ok(Some(decoded));
                    }
                }
            }
        }
    }

    /// How many bytes of the body are still to come, when its length is
    /// known and nothing decodes it.
    pub(crate) fn left(&self) -> Option<usize> {
        match (&self.incoming, &self.decoder) {
            (_, Some(_)) => None,
            (Some(incoming), None) => usize::try_from(hyper::body::Body::size_hint(incoming).exact()?).ok(),
            (None, None) => Some(0),
        }
    }

    /// The whole body.
    pub(crate) async fn all(mut self) -> Result<Bytes, Failure> {
        let mut whole = bytes::BytesMut::new();
        while let Some(chunk) = self.chunk().await? {
            whole.extend_from_slice(&chunk);
        }
        Ok(whole.freeze())
    }
}

/// What decodes a body of `Content-Encoding` gzip, deflate (zlib's, as
/// HTTP means it) or br, as the bytes come.
enum Decoder {
    Gzip(flate2::write::GzDecoder<Vec<u8>>),
    Zlib(flate2::write::ZlibDecoder<Vec<u8>>),
    Brotli(Box<brotli_decompressor::DecompressorWriter<Vec<u8>>>),
}

impl Decoder {
    fn of(encoding: &str) -> Option<Decoder> {
        match encoding.trim().to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => Some(Decoder::Gzip(flate2::write::GzDecoder::new(Vec::new()))),
            "deflate" => Some(Decoder::Zlib(flate2::write::ZlibDecoder::new(Vec::new()))),
            "br" => Some(Decoder::Brotli(Box::new(brotli_decompressor::DecompressorWriter::new(Vec::new(), 4096)))),
            _ => None,
        }
    }

    /// What the bytes so far decode to.
    fn feed(&mut self, data: &[u8]) -> Result<Bytes, Failure> {
        use std::io::Write;
        let wrong = |error: std::io::Error| failed("body", format!("the body does not decode: {error}"));
        let output = match self {
            Decoder::Gzip(decoder) => {
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(wrong)?;
                decoder.get_mut()
            }
            Decoder::Zlib(decoder) => {
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(wrong)?;
                decoder.get_mut()
            }
            Decoder::Brotli(decoder) => {
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(wrong)?;
                decoder.get_mut()
            }
        };
        Ok(Bytes::from(std::mem::take(output)))
    }

    /// What is left once the body ended, which must end the encoding too.
    fn finish(self) -> Result<Bytes, Failure> {
        let wrong = |error: std::io::Error| failed("body", format!("the body does not decode: {error}"));
        let rest = match self {
            Decoder::Gzip(decoder) => decoder.finish().map_err(wrong)?,
            Decoder::Zlib(decoder) => decoder.finish().map_err(wrong)?,
            Decoder::Brotli(decoder) => {
                decoder.into_inner().map_err(|_| failed("body", "the body ends before its brotli does"))?
            }
        };
        Ok(Bytes::from(rest))
    }
}

/// A chunk of a body that is read as it is sent.
type Chunks = std::pin::Pin<Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send>>;

/// A body read from chunks as it is sent.
pub(crate) fn streamed(chunks: impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static) -> Outgoing {
    use futures_util::TryStreamExt;
    http_body_util::StreamBody::new(chunks.map_ok(hyper::body::Frame::data)).boxed_unsync()
}

/// A multipart form (RFC 7578), its parts in order.
#[derive(Default)]
pub(crate) struct Form {
    parts: Vec<Part>,
}

pub(crate) struct Part {
    pub(crate) name: String,
    pub(crate) file_name: String,
    pub(crate) content_type: String,
    pub(crate) content: Content,
}

pub(crate) enum Content {
    Bytes(Bytes),
    /// Read as it is sent, of its length when it is known.
    Stream(Chunks, Option<u64>),
}

impl Form {
    pub(crate) fn add(&mut self, part: Part) {
        self.parts.push(part);
    }

    /// The form as a body: its chunks, its length when every part has one,
    /// and its content type, with the boundary.
    fn into_body(self) -> (Outgoing, Option<u64>, String) {
        use futures_util::StreamExt;
        let boundary = boundary();
        let mut length = Some(0u64);
        let mut pieces: Vec<Chunks> = Vec::new();
        let text = |text: String, length: &mut Option<u64>, pieces: &mut Vec<Chunks>| {
            *length = length.map(|length| length + text.len() as u64);
            pieces.push(Box::pin(futures_util::stream::once(async move { Ok(Bytes::from(text)) })));
        };
        for part in self.parts {
            let mut head = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"", quoted(&part.name));
            if !part.file_name.is_empty() {
                head.push_str(&format!("; filename=\"{}\"", quoted(&part.file_name)));
            }
            if !part.content_type.is_empty() {
                head.push_str(&format!("\r\nContent-Type: {}", part.content_type));
            }
            head.push_str("\r\n\r\n");
            text(head, &mut length, &mut pieces);
            match part.content {
                Content::Bytes(data) => {
                    length = length.map(|length| length + data.len() as u64);
                    pieces.push(Box::pin(futures_util::stream::once(async move { Ok(data) })));
                }
                Content::Stream(chunks, size) => {
                    length = length.zip(size).map(|(length, size)| length + size);
                    pieces.push(chunks);
                }
            }
            text("\r\n".to_string(), &mut length, &mut pieces);
        }
        text(format!("--{boundary}--\r\n"), &mut length, &mut pieces);
        let body = streamed(futures_util::stream::iter(pieces).flatten());
        (body, length, format!("multipart/form-data; boundary={boundary}"))
    }
}

/// A name in a part's head, with what would end it written as browsers
/// write it.
fn quoted(name: &str) -> String {
    name.replace('"', "%22").replace('\r', "%0D").replace('\n', "%0A")
}

/// A boundary no part holds, but by a chance of one in 2^128.
fn boundary() -> String {
    use ring::rand::SecureRandom;
    let mut random = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut random).expect("the system's entropy");
    random.iter().map(|byte| format!("{byte:02x}")).collect()
}
