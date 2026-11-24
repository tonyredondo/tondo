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
pub(crate) enum Outgoing {
    Empty,
    Full(Option<Bytes>),
    Stream(http_body_util::combinators::UnsyncBoxBody<Bytes, std::io::Error>),
}

impl hyper::body::Body for Outgoing {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, std::io::Error>>> {
        match self.get_mut() {
            Outgoing::Empty => std::task::Poll::Ready(None),
            Outgoing::Full(bytes) => {
                std::task::Poll::Ready(bytes.take().filter(|b| !b.is_empty()).map(|b| Ok(hyper::body::Frame::data(b))))
            }
            Outgoing::Stream(stream) => std::pin::Pin::new(stream).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Outgoing::Empty => true,
            Outgoing::Full(bytes) => bytes.as_ref().is_none_or(Bytes::is_empty),
            Outgoing::Stream(stream) => stream.is_end_stream(),
        }
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        match self {
            Outgoing::Empty => hyper::body::SizeHint::with_exact(0),
            Outgoing::Full(bytes) => hyper::body::SizeHint::with_exact(bytes.as_ref().map_or(0, Bytes::len) as u64),
            Outgoing::Stream(stream) => stream.size_hint(),
        }
    }
}

/// Static header value when `bytes` matches a common fixed value.
pub(crate) fn static_header_value(bytes: &[u8]) -> Option<hyper::header::HeaderValue> {
    const AGENT: &str = concat!("tondo/", env!("CARGO_PKG_VERSION"));
    let text = match bytes.len() {
        1 if bytes == b"0" => "0",
        2 if bytes == b"br" => "br",
        4 if bytes == b"gzip" => "gzip",
        5 if bytes == b"bytes" => "bytes",
        5 if bytes == b"close" => "close",
        7 if bytes == b"chunked" => "chunked",
        7 if bytes == b"deflate" => "deflate",
        8 if bytes == b"no-cache" => "no-cache",
        9 if bytes == b"text/html" => "text/html",
        10 if bytes == b"keep-alive" => "keep-alive",
        10 if bytes == b"text/plain" => "text/plain",
        16 if bytes == b"application/json" => "application/json",
        16 if bytes == b"gzip, deflate, br" => "gzip, deflate, br",
        17 if bytes == b"text/event-stream" => "text/event-stream",
        24 if bytes == b"text/html; charset=utf-8" => "text/html; charset=utf-8",
        24 if bytes == b"application/octet-stream" => "application/octet-stream",
        24 if bytes == b"application/problem+json" => "application/problem+json",
        25 if bytes == b"text/plain; charset=utf-8" => "text/plain; charset=utf-8",
        31 if bytes == b"application/json; charset=utf-8" => "application/json; charset=utf-8",
        33 if bytes == b"application/x-www-form-urlencoded" => "application/x-www-form-urlencoded",
        n if n == AGENT.len() && bytes == AGENT.as_bytes() => AGENT,
        _ => return None,
    };
    Some(hyper::header::HeaderValue::from_static(text))
}

/// Static header values for common headers, avoiding a heap copy on each request.
pub(crate) fn header_value(value: &str) -> Result<hyper::header::HeaderValue, hyper::header::InvalidHeaderValue> {
    if let Some(kept) = static_header_value(value.as_bytes()) {
        return Ok(kept);
    }
    hyper::header::HeaderValue::try_from(value)
}

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
        let proxy = if secure { self.https.as_ref() } else { self.http.as_ref() }?;
        if self.bypass.is_empty() {
            return Some(proxy);
        }
        let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
        let bypassed = self.bypass.iter().any(|entry| {
            let entry = entry.trim_start_matches('.');
            entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
        });
        (!bypassed).then_some(proxy)
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

/// Response headers in arrival order: much smaller than a `HeaderMap` hash
/// table for typical responses with a handful of headers.
pub(crate) type ResponseHeaders = Vec<(hyper::header::HeaderName, hyper::header::HeaderValue)>;

fn headers_from_map(map: hyper::HeaderMap) -> ResponseHeaders {
    let mut out = Vec::with_capacity(map.len());
    let mut current: Option<hyper::header::HeaderName> = None;
    for (name, value) in map {
        if let Some(name) = name {
            current = Some(name);
        }
        if let Some(name) = &current {
            out.push((name.clone(), value));
        }
    }
    out
}

/// Borrowed lookup key for `Route`, avoiding a `host: String` allocation on
/// every keep-alive request.
struct RouteRef<'a> {
    secure: bool,
    host: &'a str,
    port: u16,
    via: ViaRef<'a>,
}

enum ViaRef<'a> {
    Direct,
    Forward(&'a Proxy),
    Tunnel(&'a Proxy),
    Socket(&'a std::path::Path),
}

impl RouteRef<'_> {
    fn hash_key(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.secure.hash(&mut h);
        self.host.hash(&mut h);
        self.port.hash(&mut h);
        match &self.via {
            ViaRef::Direct => 0u8.hash(&mut h),
            ViaRef::Forward(p) => {
                1u8.hash(&mut h);
                p.hash(&mut h);
            }
            ViaRef::Tunnel(p) => {
                2u8.hash(&mut h);
                p.hash(&mut h);
            }
            ViaRef::Socket(path) => {
                3u8.hash(&mut h);
                path.hash(&mut h);
            }
        }
        h.finish()
    }

    fn matches(&self, route: &Route) -> bool {
        if self.secure != route.secure || self.port != route.port || self.host != route.host {
            return false;
        }
        match (&self.via, &route.via) {
            (ViaRef::Direct, Via::Direct) => true,
            (ViaRef::Forward(a), Via::Forward(b)) => *a == b,
            (ViaRef::Tunnel(a), Via::Tunnel(b)) => *a == b,
            (ViaRef::Socket(a), Via::Socket(b)) => *a == b.as_path(),
            _ => false,
        }
    }

    fn to_owned(&self) -> Route {
        let via = match &self.via {
            ViaRef::Direct => Via::Direct,
            ViaRef::Forward(p) => Via::Forward((*p).clone()),
            ViaRef::Tunnel(p) => Via::Tunnel((*p).clone()),
            ViaRef::Socket(path) => Via::Socket(path.to_path_buf()),
        };
        Route { secure: self.secure, host: self.host.to_owned(), port: self.port, via }
    }
}

/// Reusable-buffer HTTP/1.1 client connection. Unlike hyper's HTTP/1 client
/// (whose `maybe_notify` calls `read_buf.reserve(8192)` while the response
/// body `Bytes` slice is still live, allocating a fresh 8 KiB buffer on every
/// response), this keeps one persistent `read_buf` and `write_buf` for the
/// lifetime of the connection.
struct Http1 {
    io: Box<dyn Duplex>,
    write_buf: Vec<u8>,
    read_buf: Vec<u8>,
    read_pos: usize,
}

const H1_INIT_READ_BUF: usize = 8192;
const H1_MAX_HEAD_BYTES: usize = 409_600;
const H1_MAX_HEADERS: usize = 100;
const H1_INLINE_BODY_MAX: usize = 64 * 1024;

impl Http1 {
    fn new(io: Box<dyn Duplex>) -> Http1 {
        Http1 { io, write_buf: Vec::new(), read_buf: Vec::with_capacity(H1_INIT_READ_BUF), read_pos: 0 }
    }

    fn unread(&self) -> &[u8] {
        &self.read_buf[self.read_pos..]
    }

    fn consume(&mut self, count: usize) {
        self.read_pos += count;
        if self.read_pos >= self.read_buf.len() {
            self.read_pos = 0;
            self.read_buf.clear();
        }
    }

    fn compact_if_needed(&mut self) {
        if self.read_pos > 0 {
            let rem = self.read_buf.len() - self.read_pos;
            if rem > 0 {
                self.read_buf.copy_within(self.read_pos.., 0);
            }
            self.read_buf.truncate(rem);
            self.read_pos = 0;
        }
    }

    /// Checks without blocking whether an idle keep-alive connection is still
    /// open and free of unexpected bytes.
    fn is_alive(&mut self) -> bool {
        if self.read_pos < self.read_buf.len() {
            return false;
        }
        let mut byte = [0u8; 1];
        let mut buf = tokio::io::ReadBuf::new(&mut byte);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        matches!(std::pin::Pin::new(&mut *self.io).poll_read(&mut cx, &mut buf), std::task::Poll::Pending)
    }

    /// Reads more bytes into `read_buf` without zero-initializing spare space.
    fn poll_read_more(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<std::io::Result<usize>> {
        if self.read_pos == self.read_buf.len() {
            self.read_pos = 0;
            self.read_buf.clear();
        } else if self.read_buf.capacity() - self.read_buf.len() < 4096 && self.read_pos > 0 {
            self.compact_if_needed();
        }
        if self.read_buf.capacity() - self.read_buf.len() < 4096 {
            self.read_buf.reserve(4096);
        }
        let spare = self.read_buf.spare_capacity_mut();
        let mut buf = tokio::io::ReadBuf::uninit(spare);
        match std::pin::Pin::new(&mut *self.io).poll_read(cx, &mut buf) {
            std::task::Poll::Ready(Ok(())) => {
                let n = buf.filled().len();
                unsafe {
                    let new_len = self.read_buf.len() + n;
                    self.read_buf.set_len(new_len);
                }
                std::task::Poll::Ready(Ok(n))
            }
            std::task::Poll::Ready(Err(e)) => std::task::Poll::Ready(Err(e)),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    async fn read_more(&mut self) -> std::io::Result<usize> {
        std::future::poll_fn(|cx| self.poll_read_more(cx)).await
    }

    async fn write_all_buf(io: &mut dyn Duplex, mut data: &[u8]) -> std::io::Result<()> {
        while !data.is_empty() {
            let n = std::future::poll_fn(|cx| std::pin::Pin::new(&mut *io).poll_write(cx, data)).await?;
            if n == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            data = &data[n..];
        }
        Ok(())
    }

    async fn flush_io(io: &mut dyn Duplex) -> std::io::Result<()> {
        std::future::poll_fn(|cx| std::pin::Pin::new(&mut *io).poll_flush(cx)).await
    }
}

fn valid_header_name(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes.iter().all(|&b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

fn valid_header_value(bytes: &[u8]) -> bool {
    bytes.iter().all(|&b| b == b'\t' || (b >= 32 && b != 127))
}

fn has_token_ignore_case(value: &[u8], token: &[u8]) -> bool {
    value.split(|&b| b == b',').any(|part| trim_ows(part).eq_ignore_ascii_case(token))
}

fn trim_ows(mut bytes: &[u8]) -> &[u8] {
    while let Some(&first) = bytes.first() {
        if first == b' ' || first == b'\t' {
            bytes = &bytes[1..];
        } else {
            break;
        }
    }
    while let Some(&last) = bytes.last() {
        if last == b' ' || last == b'\t' {
            bytes = &bytes[..bytes.len() - 1];
        } else {
            break;
        }
    }
    bytes
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    let mut start = 0;
    while let Some(pos) = memchr::memchr(b'\n', &buf[start..]) {
        let i = start + pos;
        if i >= 3 && buf[i - 3] == b'\r' && buf[i - 2] == b'\n' && buf[i - 1] == b'\r' {
            return Some(i + 1);
        }
        if i >= 1 && buf[i - 1] == b'\n' {
            return Some(i + 1);
        }
        start = i + 1;
    }
    None
}

fn find_crlf(buf: &[u8]) -> Option<(usize, usize)> {
    let pos = memchr::memchr(b'\n', buf)?;
    if pos > 0 && buf[pos - 1] == b'\r' { Some((pos - 1, pos + 1)) } else { Some((pos, pos + 1)) }
}

enum H1Framing {
    Empty,
    Length(u64),
    Chunked,
    UntilEof,
}

struct H1Head {
    status: hyper::StatusCode,
    version: hyper::Version,
    headers: ResponseHeaders,
    framing: H1Framing,
    keep_alive: bool,
}

enum H1Error {
    IoBeforeResponse(std::io::Error),
    Protocol(String),
}

struct H1Request<'a> {
    method: &'a hyper::Method,
    url: &'a url::Url,
    route: &'a RouteRef<'a>,
    headers: &'a [(String, String)],
    outgoing: Outgoing,
    length: Option<u64>,
    kind: Option<String>,
    cookies: &'a Option<Mutex<cookie_store::CookieStore>>,
    decodes: bool,
}

impl Http1 {
    async fn send_request_and_read_head(&mut self, req: H1Request<'_>) -> Result<H1Head, H1Error> {
        use std::io::Write as _;
        let H1Request { method, url, route, headers, outgoing, length, kind, cookies, decodes } = req;

        let has = |name: &str| headers.iter().any(|(given, _)| given.eq_ignore_ascii_case(name));
        let mut req_close = false;
        let has_te = has("transfer-encoding");
        let has_cl = has("content-length");
        let chunked_request = matches!(&outgoing, Outgoing::Stream(_)) && length.is_none() && !has_cl && !has_te;

        self.write_buf.clear();
        let buf = &mut self.write_buf;
        buf.extend_from_slice(method.as_str().as_bytes());
        buf.push(b' ');
        match &route.via {
            ViaRef::Forward(_) => buf.extend_from_slice(url.as_str().as_bytes()),
            _ => {
                buf.extend_from_slice(url.path().as_bytes());
                if let Some(query) = url.query() {
                    buf.push(b'?');
                    buf.extend_from_slice(query.as_bytes());
                }
            }
        }
        buf.extend_from_slice(b" HTTP/1.1\r\n");

        if !has("host") {
            buf.extend_from_slice(b"host: ");
            buf.extend_from_slice(route.host.as_bytes());
            if let Some(port) = url.port() {
                let _ = write!(buf, ":{port}");
            }
            buf.extend_from_slice(b"\r\n");
        }
        if let ViaRef::Forward(Proxy { authorization: Some(authorization), .. }) = &route.via {
            buf.extend_from_slice(b"proxy-authorization: ");
            buf.extend_from_slice(authorization.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        if !has("user-agent") {
            buf.extend_from_slice(concat!("user-agent: tondo/", env!("CARGO_PKG_VERSION"), "\r\n").as_bytes());
        }
        if decodes {
            buf.extend_from_slice(b"accept-encoding: gzip, deflate, br\r\n");
        }
        if let Some(cookies) = cookies
            && !has("cookie")
        {
            let jar = cookies.lock().unwrap_or_else(|e| e.into_inner());
            let mut first = true;
            for (name, value) in jar.get_request_values(url) {
                if first {
                    buf.extend_from_slice(b"cookie: ");
                    first = false;
                } else {
                    buf.extend_from_slice(b"; ");
                }
                buf.extend_from_slice(name.as_bytes());
                buf.push(b'=');
                buf.extend_from_slice(value.as_bytes());
            }
            if !first {
                buf.extend_from_slice(b"\r\n");
            }
        }
        for (name, value) in headers {
            if name.eq_ignore_ascii_case("connection") && has_token_ignore_case(value.as_bytes(), b"close") {
                req_close = true;
            }
            buf.extend_from_slice(name.as_bytes());
            buf.extend_from_slice(b": ");
            buf.extend_from_slice(value.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        if let Some(kind) = kind.filter(|kind| !kind.is_empty() && !has("content-type")) {
            buf.extend_from_slice(b"content-type: ");
            buf.extend_from_slice(kind.as_bytes());
            buf.extend_from_slice(b"\r\n");
        }
        if let Some(length) = length.filter(|_| !has_cl && !has_te) {
            let _ = write!(buf, "content-length: {length}\r\n");
        } else if chunked_request {
            buf.extend_from_slice(b"transfer-encoding: chunked\r\n");
        }
        buf.extend_from_slice(b"\r\n");

        match outgoing {
            Outgoing::Empty => {
                Self::write_all_buf(&mut *self.io, &self.write_buf).await.map_err(H1Error::IoBeforeResponse)?;
            }
            Outgoing::Full(Some(data)) => {
                if data.len() <= 16_384 {
                    self.write_buf.extend_from_slice(&data);
                    Self::write_all_buf(&mut *self.io, &self.write_buf).await.map_err(H1Error::IoBeforeResponse)?;
                } else {
                    Self::write_all_buf(&mut *self.io, &self.write_buf).await.map_err(H1Error::IoBeforeResponse)?;
                    Self::write_all_buf(&mut *self.io, &data).await.map_err(H1Error::IoBeforeResponse)?;
                }
            }
            Outgoing::Full(None) => {
                Self::write_all_buf(&mut *self.io, &self.write_buf).await.map_err(H1Error::IoBeforeResponse)?;
            }
            Outgoing::Stream(mut stream) => {
                Self::write_all_buf(&mut *self.io, &self.write_buf).await.map_err(H1Error::IoBeforeResponse)?;
                while let Some(frame) =
                    std::future::poll_fn(|cx| hyper::body::Body::poll_frame(std::pin::Pin::new(&mut stream), cx)).await
                {
                    let frame = frame.map_err(|e| H1Error::Protocol(with_causes(&e)))?;
                    let Ok(data) = frame.into_data() else { continue };
                    if data.is_empty() {
                        continue;
                    }
                    if chunked_request {
                        self.write_buf.clear();
                        let _ = write!(&mut self.write_buf, "{:x}\r\n", data.len());
                        if data.len() <= 16_384 {
                            self.write_buf.extend_from_slice(&data);
                            self.write_buf.extend_from_slice(b"\r\n");
                            Self::write_all_buf(&mut *self.io, &self.write_buf)
                                .await
                                .map_err(H1Error::IoBeforeResponse)?;
                        } else {
                            Self::write_all_buf(&mut *self.io, &self.write_buf)
                                .await
                                .map_err(H1Error::IoBeforeResponse)?;
                            Self::write_all_buf(&mut *self.io, &data).await.map_err(H1Error::IoBeforeResponse)?;
                            Self::write_all_buf(&mut *self.io, b"\r\n").await.map_err(H1Error::IoBeforeResponse)?;
                        }
                    } else {
                        Self::write_all_buf(&mut *self.io, &data).await.map_err(H1Error::IoBeforeResponse)?;
                    }
                }
                if chunked_request {
                    Self::write_all_buf(&mut *self.io, b"0\r\n\r\n").await.map_err(H1Error::IoBeforeResponse)?;
                }
            }
        }
        Self::flush_io(&mut *self.io).await.map_err(H1Error::IoBeforeResponse)?;

        let mut received_any = !self.unread().is_empty();
        loop {
            let head_end = loop {
                if let Some(end) = find_header_end(self.unread()) {
                    break end;
                }
                if self.unread().len() >= H1_MAX_HEAD_BYTES {
                    return Err(H1Error::Protocol("response headers are too large".into()));
                }
                let n = match self.read_more().await {
                    Ok(n) => n,
                    Err(e) if !received_any => return Err(H1Error::IoBeforeResponse(e)),
                    Err(e) => return Err(H1Error::Protocol(e.to_string())),
                };
                if n == 0 {
                    if !received_any {
                        return Err(H1Error::IoBeforeResponse(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "connection closed before message completed",
                        )));
                    }
                    return Err(H1Error::Protocol("connection closed before message completed".into()));
                }
                received_any = true;
            };

            let head_bytes = &self.unread()[..head_end];
            let (line_end, mut cursor) =
                find_crlf(head_bytes).ok_or_else(|| H1Error::Protocol("invalid status line".into()))?;
            let status_line = &head_bytes[..line_end];
            if status_line.len() < 12 || !status_line.starts_with(b"HTTP/1.") || status_line[8] != b' ' {
                return Err(H1Error::Protocol("invalid HTTP response status line".into()));
            }
            let version = match status_line[7] {
                b'1' => hyper::Version::HTTP_11,
                b'0' => hyper::Version::HTTP_10,
                _ => return Err(H1Error::Protocol("unsupported HTTP version".into())),
            };
            if status_line.len() > 12 && status_line[12] != b' ' {
                return Err(H1Error::Protocol("invalid HTTP response status code".into()));
            }
            let status = hyper::StatusCode::from_bytes(&status_line[9..12])
                .map_err(|_| H1Error::Protocol("invalid HTTP status code".into()))?;

            let mut parsed_headers = Vec::with_capacity(8);
            let mut content_length: Option<u64> = None;
            let mut has_te = false;
            let mut te_chunked = false;
            let mut conn_close = false;
            let mut conn_keep_alive = false;

            while cursor < head_bytes.len() {
                let Some((rel_end, rel_next)) = find_crlf(&head_bytes[cursor..]) else {
                    break;
                };
                let line = &head_bytes[cursor..cursor + rel_end];
                cursor += rel_next;
                if line.is_empty() {
                    break;
                }
                if parsed_headers.len() >= H1_MAX_HEADERS {
                    return Err(H1Error::Protocol("too many response headers".into()));
                }
                let colon =
                    memchr::memchr(b':', line).ok_or_else(|| H1Error::Protocol("malformed response header".into()))?;
                let name = hyper::header::HeaderName::from_bytes(&line[..colon])
                    .map_err(|_| H1Error::Protocol("invalid response header name".into()))?;
                let raw_value = trim_ows(&line[colon + 1..]);
                let value = match static_header_value(raw_value) {
                    Some(v) => v,
                    None => hyper::header::HeaderValue::from_bytes(raw_value)
                        .map_err(|_| H1Error::Protocol("invalid response header value".into()))?,
                };
                match name {
                    hyper::header::TRANSFER_ENCODING => {
                        has_te = true;
                        te_chunked = raw_value
                            .rsplit(|&b| b == b',')
                            .next()
                            .is_some_and(|last| trim_ows(last).eq_ignore_ascii_case(b"chunked"));
                    }
                    hyper::header::CONTENT_LENGTH => {
                        for part in raw_value.split(|&b| b == b',') {
                            let part = trim_ows(part);
                            let parsed = std::str::from_utf8(part)
                                .ok()
                                .and_then(|s| s.parse::<u64>().ok())
                                .ok_or_else(|| H1Error::Protocol("invalid Content-Length header".into()))?;
                            if let Some(prev) = content_length
                                && prev != parsed
                            {
                                return Err(H1Error::Protocol("conflicting Content-Length headers".into()));
                            }
                            content_length = Some(parsed);
                        }
                    }
                    hyper::header::CONNECTION => {
                        if has_token_ignore_case(raw_value, b"close") {
                            conn_close = true;
                        }
                        if has_token_ignore_case(raw_value, b"keep-alive") {
                            conn_keep_alive = true;
                        }
                    }
                    _ => {}
                }
                parsed_headers.push((name, value));
            }

            self.consume(head_end);

            // Skip 1xx informational responses (except 101 Switching Protocols).
            if status.is_informational() && status != hyper::StatusCode::SWITCHING_PROTOCOLS {
                continue;
            }

            let framing = if *method == hyper::Method::HEAD
                || status == hyper::StatusCode::NO_CONTENT
                || status == hyper::StatusCode::NOT_MODIFIED
                || status.is_informational()
            {
                H1Framing::Empty
            } else if has_te {
                if version == hyper::Version::HTTP_10 {
                    return Err(H1Error::Protocol("HTTP/1.0 response has Transfer-Encoding".into()));
                }
                if te_chunked { H1Framing::Chunked } else { H1Framing::UntilEof }
            } else if let Some(len) = content_length {
                H1Framing::Length(len)
            } else {
                H1Framing::UntilEof
            };

            let keep_alive = !req_close
                && !conn_close
                && !matches!(framing, H1Framing::UntilEof)
                && !(has_te && content_length.is_some())
                && (version == hyper::Version::HTTP_11 || (version == hyper::Version::HTTP_10 && conn_keep_alive));

            return Ok(H1Head { status, version, headers: parsed_headers, framing, keep_alive });
        }
    }
}

enum ChunkedState {
    SizeLine,
    Data(u64),
    DataCrLf,
    Trailers,
}

enum H1BodyState {
    Length(u64),
    Chunked(ChunkedState),
    UntilEof,
    Done,
}

struct Http1Body {
    conn: Option<Http1>,
    state: H1BodyState,
    return_to: Option<(std::sync::Weak<IdlePool>, Route)>,
}

impl Http1Body {
    fn finish(&mut self, reusable: bool) {
        self.state = H1BodyState::Done;
        if let Some(conn) = self.conn.take()
            && reusable
            && conn.unread().is_empty()
            && let Some((pool, route)) = self.return_to.take()
            && let Some(pool) = pool.upgrade()
        {
            keep_in_pool(&pool, route, Sender::one(conn));
        }
    }

    fn poll_chunk(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<Option<Bytes>, Failure>> {
        use std::task::Poll;
        loop {
            match &mut self.state {
                H1BodyState::Done => return Poll::Ready(Ok(None)),
                H1BodyState::Length(0) => {
                    self.finish(true);
                    return Poll::Ready(Ok(None));
                }
                H1BodyState::Length(rem) => {
                    let Some(conn) = self.conn.as_mut() else {
                        self.state = H1BodyState::Done;
                        return Poll::Ready(Ok(None));
                    };
                    if conn.unread().is_empty() {
                        match conn.poll_read_more(cx) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(Ok(0)) => {
                                self.finish(false);
                                return Poll::Ready(Err(failed("body", "unexpected EOF while reading body")));
                            }
                            Poll::Ready(Ok(_)) => {}
                            Poll::Ready(Err(e)) => {
                                self.finish(false);
                                return Poll::Ready(Err(failed("body", e)));
                            }
                        }
                    }
                    let take = (*rem).min(conn.unread().len() as u64) as usize;
                    let chunk = Bytes::copy_from_slice(&conn.unread()[..take]);
                    conn.consume(take);
                    *rem -= take as u64;
                    if *rem == 0 {
                        self.finish(true);
                    }
                    return Poll::Ready(Ok(Some(chunk)));
                }
                H1BodyState::UntilEof => {
                    let Some(conn) = self.conn.as_mut() else {
                        self.state = H1BodyState::Done;
                        return Poll::Ready(Ok(None));
                    };
                    if conn.unread().is_empty() {
                        match conn.poll_read_more(cx) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(Ok(0)) => {
                                self.finish(false);
                                return Poll::Ready(Ok(None));
                            }
                            Poll::Ready(Ok(_)) => {}
                            Poll::Ready(Err(e)) => {
                                self.finish(false);
                                return Poll::Ready(Err(failed("body", e)));
                            }
                        }
                    }
                    let take = conn.unread().len();
                    let chunk = Bytes::copy_from_slice(&conn.unread()[..take]);
                    conn.consume(take);
                    return Poll::Ready(Ok(Some(chunk)));
                }
                H1BodyState::Chunked(chunked) => {
                    let Some(conn) = self.conn.as_mut() else {
                        self.state = H1BodyState::Done;
                        return Poll::Ready(Ok(None));
                    };
                    match chunked {
                        ChunkedState::SizeLine => {
                            let Some((line_end, next)) = find_crlf(conn.unread()) else {
                                if conn.unread().len() > 8192 {
                                    self.finish(false);
                                    return Poll::Ready(Err(failed("body", "chunk size line is too large")));
                                }
                                match conn.poll_read_more(cx) {
                                    Poll::Pending => return Poll::Pending,
                                    Poll::Ready(Ok(0)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", "unexpected EOF in chunked body")));
                                    }
                                    Poll::Ready(Ok(_)) => continue,
                                    Poll::Ready(Err(e)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", e)));
                                    }
                                }
                            };
                            let line = &conn.unread()[..line_end];
                            let size_part = match memchr::memchr(b';', line) {
                                Some(semi) => &line[..semi],
                                None => line,
                            };
                            let size_str = match std::str::from_utf8(trim_ows(size_part)) {
                                Ok(s) => s,
                                Err(_) => {
                                    self.finish(false);
                                    return Poll::Ready(Err(failed("body", "invalid chunk size")));
                                }
                            };
                            let Ok(size) = u64::from_str_radix(size_str, 16) else {
                                self.finish(false);
                                return Poll::Ready(Err(failed("body", "invalid chunk size")));
                            };
                            conn.consume(next);
                            if size == 0 {
                                *chunked = ChunkedState::Trailers;
                            } else {
                                *chunked = ChunkedState::Data(size);
                            }
                        }
                        ChunkedState::Data(rem) => {
                            if conn.unread().is_empty() {
                                match conn.poll_read_more(cx) {
                                    Poll::Pending => return Poll::Pending,
                                    Poll::Ready(Ok(0)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", "unexpected EOF in chunked body")));
                                    }
                                    Poll::Ready(Ok(_)) => {}
                                    Poll::Ready(Err(e)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", e)));
                                    }
                                }
                            }
                            let take = (*rem).min(conn.unread().len() as u64) as usize;
                            let data = Bytes::copy_from_slice(&conn.unread()[..take]);
                            conn.consume(take);
                            *rem -= take as u64;
                            if *rem == 0 {
                                *chunked = ChunkedState::DataCrLf;
                            }
                            return Poll::Ready(Ok(Some(data)));
                        }
                        ChunkedState::DataCrLf => {
                            let unread = conn.unread();
                            if unread.len() < 2 {
                                match conn.poll_read_more(cx) {
                                    Poll::Pending => return Poll::Pending,
                                    Poll::Ready(Ok(0)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", "unexpected EOF in chunked body")));
                                    }
                                    Poll::Ready(Ok(_)) => continue,
                                    Poll::Ready(Err(e)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", e)));
                                    }
                                }
                            }
                            if &unread[..2] != b"\r\n" {
                                self.finish(false);
                                return Poll::Ready(Err(failed("body", "invalid chunk terminator")));
                            }
                            conn.consume(2);
                            *chunked = ChunkedState::SizeLine;
                        }
                        ChunkedState::Trailers => loop {
                            let Some((line_end, next)) = find_crlf(conn.unread()) else {
                                if conn.unread().len() > H1_MAX_HEAD_BYTES {
                                    self.finish(false);
                                    return Poll::Ready(Err(failed("body", "chunked trailers are too large")));
                                }
                                match conn.poll_read_more(cx) {
                                    Poll::Pending => return Poll::Pending,
                                    Poll::Ready(Ok(0)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", "unexpected EOF in chunked trailers")));
                                    }
                                    Poll::Ready(Ok(_)) => break,
                                    Poll::Ready(Err(e)) => {
                                        self.finish(false);
                                        return Poll::Ready(Err(failed("body", e)));
                                    }
                                }
                            };
                            conn.consume(next);
                            if line_end == 0 {
                                self.finish(true);
                                return Poll::Ready(Ok(None));
                            }
                        },
                    }
                }
            }
        }
    }
}

/// A connection: HTTP/1 with persistent reusable buffers, or HTTP/2 via hyper
/// whose driver future runs at the workers' turns and is polled in place too.
enum Sender {
    One(Box<Http1>),
    Two(hyper::client::conn::http2::SendRequest<Outgoing>, Arc<crate::server::Turning>),
}

impl Sender {
    fn one(conn: Http1) -> Sender {
        Sender::One(Box::new(conn))
    }

    fn shared(&self) -> Option<Sender> {
        match self {
            Sender::Two(sender, connection) => Some(Sender::Two(sender.clone(), connection.clone())),
            Sender::One(_) => None,
        }
    }
}

type IdlePool = Mutex<HashMap<u64, Vec<(Route, Vec<Sender>)>>>;

fn keep_in_pool(idle: &IdlePool, route: Route, sender: Sender) {
    let key = RouteRef {
        secure: route.secure,
        host: &route.host,
        port: route.port,
        via: match &route.via {
            Via::Direct => ViaRef::Direct,
            Via::Forward(p) => ViaRef::Forward(p),
            Via::Tunnel(p) => ViaRef::Tunnel(p),
            Via::Socket(p) => ViaRef::Socket(p),
        },
    }
    .hash_key();
    let mut map = idle.lock().unwrap_or_else(|e| e.into_inner());
    let bucket = map.entry(key).or_default();
    if let Some((_, kept)) = bucket.iter_mut().find(|(existing, _)| *existing == route) {
        if kept.len() < IDLE {
            kept.push(sender);
        }
        return;
    }
    bucket.push((route, vec![sender]));
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
    idle: Arc<IdlePool>,
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
            idle: Arc::default(),
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
    /// or a new one. Returns `(sender, reused)`.
    async fn sender(&self, route: &RouteRef<'_>) -> Result<(Sender, bool), Failure> {
        if let Some(kept) = self.kept(route) {
            return Ok((kept, true));
        }
        let owned = route.to_owned();
        let alone = owned.secure && !self.plain.lock().unwrap_or_else(|e| e.into_inner()).contains(&owned);
        let turn = alone.then(|| {
            let mut connecting = self.connecting.lock().unwrap_or_else(|e| e.into_inner());
            connecting.entry(owned.clone()).or_default().clone()
        });
        let _turn = match &turn {
            Some(turn) => {
                let turn = turn.lock().await;
                // Another made one meanwhile, which may take this request too.
                if let Some(kept) = self.kept(route) {
                    return Ok((kept, true));
                }
                Some(turn)
            }
            None => None,
        };
        let connecting = self.connect(&owned);
        let sender = match self.connect_timeout {
            Some(limit) => tokio::time::timeout(limit, connecting)
                .await
                .map_err(|_| failed("timeout", "the connection timed out"))??,
            None => connecting.await?,
        };
        match sender.shared() {
            Some(shared) => self.keep(route, shared),
            None => _ = self.plain.lock().unwrap_or_else(|e| e.into_inner()).insert(owned),
        }
        Ok((sender, false))
    }

    /// A connection of `route` the client kept that can take a request now.
    fn kept(&self, route: &RouteRef<'_>) -> Option<Sender> {
        let key = route.hash_key();
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = idle.get_mut(&key)?;
        let (_, kept) = bucket.iter_mut().find(|(existing, _)| route.matches(existing))?;
        while let Some(mut sender) = kept.pop() {
            match &mut sender {
                Sender::One(h1) => {
                    if h1.is_alive() {
                        return Some(sender);
                    }
                }
                Sender::Two(h2, _) => {
                    if !h2.is_closed() {
                        let shared = sender.shared();
                        // HTTP/2 takes many requests at once: it stays.
                        kept.push(sender);
                        return shared;
                    }
                }
            }
        }
        None
    }

    /// Keeps a connection of HTTP/1 its request let go, for the next.
    fn keep(&self, route: &RouteRef<'_>, sender: Sender) {
        let key = route.hash_key();
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let bucket = idle.entry(key).or_default();
        if let Some((_, kept)) = bucket.iter_mut().find(|(existing, _)| route.matches(existing)) {
            if kept.len() < IDLE {
                kept.push(sender);
            }
            return;
        }
        bucket.push((route.to_owned(), vec![sender]));
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
    Ok(Sender::one(Http1::new(io)))
}

async fn two(io: Box<dyn Duplex>) -> Result<Sender, Failure> {
    let (sender, connection) = hyper::client::conn::http2::handshake(Detached, hyper_util::rt::TokioIo::new(io))
        .await
        .map_err(|error| failed("connect", with_causes(&error)))?;
    let connection = crate::server::detach(async move {
        let _ = connection.await;
    });
    Ok(Sender::Two(sender, connection))
}

pub(crate) fn empty() -> Outgoing {
    Outgoing::Empty
}

/// What a request sends: its method, URL, headers and body.
pub(crate) struct Request {
    pub(crate) method: hyper::Method,
    pub(crate) url: url::Url,
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
    pub(crate) headers: ResponseHeaders,
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
        let Request { mut method, mut url, mut headers, mut body, .. } = request;
        let mut hops = 0;
        loop {
            // A body sent once is gone: bytes go again, a stream does not.
            let again = match &body {
                Body::Empty => Some(Body::Empty),
                Body::Bytes(data, kind) => Some(Body::Bytes(data.clone(), kind.clone())),
                _ => None,
            };
            let mut answer = self.exchange(&method, &url, &headers, body, deadline).await?;
            let location = answer
                .headers
                .iter()
                .find(|(name, _)| *name == hyper::header::LOCATION)
                .and_then(|(_, value)| value.to_str().ok());
            let (Some(keeps), Some(location)) = (redirects(answer.status), location) else {
                answer.url = url.into();
                return Ok(answer);
            };
            if self.redirects == 0 {
                answer.url = url.into();
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
                (true, None) => {
                    answer.url = url.into();
                    return Ok(answer);
                }
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
        mut body: Body,
        deadline: Option<Instant>,
    ) -> Result<Answer, Failure> {
        let secure = match url.scheme() {
            "http" => false,
            "https" => true,
            other => return Err(failed("url", format!("{url}: `{other}` is not http or https"))),
        };
        let host = url.host_str().ok_or_else(|| failed("url", format!("{url}: no host")))?;
        let port = url.port_or_known_default().unwrap_or(if secure { 443 } else { 80 });
        let via = match (&self.socket, self.proxies.of(secure, host)) {
            (Some(path), _) => ViaRef::Socket(path.as_path()),
            (None, Some(proxy)) if secure => ViaRef::Tunnel(proxy),
            (None, Some(proxy)) => ViaRef::Forward(proxy),
            (None, None) => ViaRef::Direct,
        };
        let route = RouteRef { secure, host, port, via };
        // Validate user headers upfront so both HTTP/1 and HTTP/2 report bad
        // headers with the same `url` failure kind.
        for (name, value) in headers {
            if !valid_header_name(name.as_bytes()) {
                return Err(failed("url", format!("invalid HTTP header name `{name}`")));
            }
            if !valid_header_value(value.as_bytes()) {
                return Err(failed("url", format!("invalid HTTP header value for `{name}`")));
            }
        }
        let has = |name: &str| headers.iter().any(|(given, _)| given.eq_ignore_ascii_case(name));
        // What this client decodes, unless the program asks for another.
        let decodes = !has("accept-encoding");
        let (mut sender, mut reused) = self.sender(&route).await?;

        let (status, version, mut response_headers, source) = loop {
            match sender {
                Sender::One(mut h1) => {
                    let retry_body = if reused {
                        match &body {
                            Body::Empty => Some(Body::Empty),
                            Body::Bytes(data, kind) => Some(Body::Bytes(data.clone(), kind.clone())),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let (outgoing, length, kind) = match body {
                        Body::Empty => (Outgoing::Empty, None, None),
                        Body::Bytes(data, kind) => {
                            let length = data.len() as u64;
                            (Outgoing::Full(Some(data)), Some(length), Some(kind))
                        }
                        Body::Stream(stream, length, kind) => (stream, length, Some(kind)),
                        Body::Form(form) => {
                            let (stream, length, kind) = form.into_body();
                            (stream, length, Some(kind))
                        }
                    };
                    if let Some(kind) = &kind
                        && !kind.is_empty()
                        && !has("content-type")
                        && !valid_header_value(kind.as_bytes())
                    {
                        return Err(failed("url", "invalid HTTP content-type header value"));
                    }
                    match h1
                        .send_request_and_read_head(H1Request {
                            method,
                            url,
                            route: &route,
                            headers,
                            outgoing,
                            length,
                            kind,
                            cookies: &self.cookies,
                            decodes,
                        })
                        .await
                    {
                        Ok(head) => {
                            let source = match head.framing {
                                H1Framing::Empty => {
                                    if head.keep_alive && h1.unread().is_empty() {
                                        self.keep(&route, Sender::One(h1));
                                    }
                                    BodySource::Ready(ReadyBody::Empty)
                                }
                                H1Framing::Length(n)
                                    if n <= H1_INLINE_BODY_MAX as u64 && h1.unread().len() >= n as usize =>
                                {
                                    let len = n as usize;
                                    let ready = ReadyBody::from_slice(&h1.unread()[..len]);
                                    h1.consume(len);
                                    if head.keep_alive && h1.unread().is_empty() {
                                        self.keep(&route, Sender::One(h1));
                                    }
                                    BodySource::Ready(ready)
                                }
                                framing => {
                                    let state = match framing {
                                        H1Framing::Length(n) => H1BodyState::Length(n),
                                        H1Framing::Chunked => H1BodyState::Chunked(ChunkedState::SizeLine),
                                        H1Framing::UntilEof => H1BodyState::UntilEof,
                                        H1Framing::Empty => unreachable!(),
                                    };
                                    let return_to =
                                        head.keep_alive.then(|| (Arc::downgrade(&self.idle), route.to_owned()));
                                    BodySource::Http1(Box::new(Http1Body { conn: Some(*h1), state, return_to }))
                                }
                            };
                            break (head.status, head.version, head.headers, source);
                        }
                        Err(H1Error::IoBeforeResponse(_)) if retry_body.is_some() => {
                            // Server closed an idle keep-alive socket before
                            // answering: open a fresh connection once.
                            body = retry_body.unwrap();
                            reused = false;
                            let owned = route.to_owned();
                            let connecting = self.connect(&owned);
                            sender = match self.connect_timeout {
                                Some(limit) => tokio::time::timeout(limit, connecting)
                                    .await
                                    .map_err(|_| failed("timeout", "the connection timed out"))??,
                                None => connecting.await?,
                            };
                            continue;
                        }
                        Err(H1Error::IoBeforeResponse(error)) => {
                            return Err(failed("protocol", format!("{method} {url}: {}", with_causes(&error))));
                        }
                        Err(H1Error::Protocol(message)) => {
                            return Err(failed("protocol", format!("{method} {url}: {message}")));
                        }
                    }
                }
                Sender::Two(mut two, connection) => {
                    let uri = hyper::Uri::try_from(url.as_str()).map_err(|error| failed("url", with_causes(&error)))?;
                    let mut builder = hyper::Request::builder().method(method.clone()).uri(uri);
                    if !has("user-agent") {
                        builder = builder.header(
                            hyper::header::USER_AGENT,
                            hyper::header::HeaderValue::from_static(concat!("tondo/", env!("CARGO_PKG_VERSION"))),
                        );
                    }
                    if decodes {
                        builder = builder.header(
                            hyper::header::ACCEPT_ENCODING,
                            hyper::header::HeaderValue::from_static("gzip, deflate, br"),
                        );
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
                        match (hyper::header::HeaderName::try_from(name.as_str()), header_value(value.as_str())) {
                            (Ok(name), Ok(value)) => builder = builder.header(name, value),
                            _ => builder = builder.header(name.as_str(), value.as_str()),
                        }
                    }
                    let (outgoing, length, kind) = match body {
                        Body::Empty => (Outgoing::Empty, None, None),
                        Body::Bytes(data, kind) => {
                            let length = data.len() as u64;
                            (Outgoing::Full(Some(data)), Some(length), Some(kind))
                        }
                        Body::Stream(stream, length, kind) => (stream, length, Some(kind)),
                        Body::Form(form) => {
                            let (stream, length, kind) = form.into_body();
                            (stream, length, Some(kind))
                        }
                    };
                    if let Some(kind) = kind.filter(|kind| !kind.is_empty() && !has("content-type")) {
                        match header_value(&kind) {
                            Ok(value) => builder = builder.header(hyper::header::CONTENT_TYPE, value),
                            Err(_) => builder = builder.header(hyper::header::CONTENT_TYPE, kind),
                        }
                    }
                    if let Some(length) = length.filter(|_| !has("content-length")) {
                        builder = builder.header(hyper::header::CONTENT_LENGTH, length);
                    }
                    let request = builder.body(outgoing).map_err(|error| failed("url", with_causes(&error)))?;
                    let response = driven(&connection, two.send_request(request))
                        .await
                        .map_err(|error| failed("protocol", format!("{method} {url}: {}", with_causes(&error))))?;
                    let (parts, incoming) = response.into_parts();
                    break (
                        parts.status,
                        parts.version,
                        headers_from_map(parts.headers),
                        BodySource::Http2 { incoming, connection },
                    );
                }
            }
        };

        if let Some(cookies) = &self.cookies {
            let parsed = response_headers
                .iter()
                .filter(|(name, _)| *name == hyper::header::SET_COOKIE)
                .filter_map(|(_, value)| value.to_str().ok())
                .filter_map(|text| cookie::Cookie::parse(text.to_owned()).ok());
            cookies.lock().unwrap_or_else(|e| e.into_inner()).store_response_cookies(parsed, url);
        }
        let encoding = response_headers
            .iter()
            .find(|(name, _)| *name == hyper::header::CONTENT_ENCODING)
            .and_then(|(_, value)| value.to_str().ok());
        let decoder = encoding.filter(|_| decodes).and_then(Decoder::of).map(Box::new);
        if decoder.is_some() {
            // The program reads what was decoded, which has other lengths.
            response_headers
                .retain(|(name, _)| *name != hyper::header::CONTENT_ENCODING && *name != hyper::header::CONTENT_LENGTH);
        }
        Ok(Answer {
            status,
            version,
            url: String::new(),
            headers: response_headers,
            body: Received { source, decoder, deadline, sleep: None, room: PASSED - 1 },
        })
    }
}

/// An HTTP/1 body that arrived in the same read as its response headers:
/// inline for small replies so reading it takes zero heap allocations.
pub(crate) enum ReadyBody {
    Empty,
    Inline { data: [u8; 24], len: u8 },
    Bytes(Bytes),
}

impl ReadyBody {
    fn from_slice(bytes: &[u8]) -> ReadyBody {
        if bytes.is_empty() {
            ReadyBody::Empty
        } else if bytes.len() <= 24 {
            let mut data = [0u8; 24];
            data[..bytes.len()].copy_from_slice(bytes);
            ReadyBody::Inline { data, len: bytes.len() as u8 }
        } else {
            ReadyBody::Bytes(Bytes::copy_from_slice(bytes))
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            ReadyBody::Empty => &[],
            ReadyBody::Inline { data, len } => &data[..usize::from(*len)],
            ReadyBody::Bytes(bytes) => bytes,
        }
    }

    fn len(&self) -> usize {
        match self {
            ReadyBody::Empty => 0,
            ReadyBody::Inline { len, .. } => usize::from(*len),
            ReadyBody::Bytes(bytes) => bytes.len(),
        }
    }

    fn into_bytes(self) -> Bytes {
        match self {
            ReadyBody::Empty => Bytes::new(),
            ReadyBody::Inline { data, len } => Bytes::copy_from_slice(&data[..usize::from(len)]),
            ReadyBody::Bytes(bytes) => bytes,
        }
    }
}

enum BodySource {
    Done,
    Ready(ReadyBody),
    Http1(Box<Http1Body>),
    Http2 { incoming: hyper::body::Incoming, connection: Arc<crate::server::Turning> },
}

/// A response's body as it comes: its chunks, decoded when the server
/// encoded them for this client, until the request's time is up.
pub(crate) struct Received {
    source: BodySource,
    decoder: Option<Box<Decoder>>,
    deadline: Option<Instant>,
    sleep: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    /// How many more bytes the body may give once decoded, of the bound its
    /// reader gives (`ClientLimits.maxBodyBytes`, or `Response.limitBody`'s),
    /// or `PASSED`. The reader, which knows the bound, words its failure.
    room: u64,
}

/// `Received.room` once the body went past its bound.
const PASSED: u64 = u64::MAX;

impl Received {
    /// The body bounded to `most` bytes once decoded.
    pub(crate) fn bounded(mut self, most: u64) -> Received {
        self.room = most.min(PASSED - 1);
        self
    }

    /// Whether the body went past its bound.
    pub(crate) fn passed(&self) -> bool {
        self.room == PASSED
    }

    /// Takes the complete body immediately when it already arrived with the
    /// response headers and needs no content decoding, avoiding an async wait.
    pub(crate) fn take_ready(&mut self) -> Option<Result<ReadyBody, Failure>> {
        if self.decoder.is_some() || self.passed() {
            return None;
        }
        match &self.source {
            BodySource::Done => Some(Ok(ReadyBody::Empty)),
            BodySource::Ready(ready) => {
                if (ready.len() as u64) > self.room {
                    self.source = BodySource::Done;
                    return Some(Err(self.past()));
                }
                let BodySource::Ready(ready) = std::mem::replace(&mut self.source, BodySource::Done) else {
                    unreachable!();
                };
                self.room -= ready.len() as u64;
                Some(Ok(ready))
            }
            BodySource::Http1(_) | BodySource::Http2 { .. } => None,
        }
    }

    /// `bytes` given, unless they pass the bound.
    fn counted(&mut self, bytes: Bytes) -> Result<Bytes, Failure> {
        match self.room.checked_sub(bytes.len() as u64) {
            Some(room) => {
                self.room = room;
                Ok(bytes)
            }
            None => Err(self.past()),
        }
    }

    /// The failure of a body past its bound, which stays so.
    fn past(&mut self) -> Failure {
        self.room = PASSED;
        failed("too large", "")
    }

    /// A failure of the decoder, which a bomb that passes the bound is.
    fn refused(&mut self, failure: Failure) -> Failure {
        if failure.0 == "too large" { self.past() } else { failure }
    }

    /// Whether reading this body has a request deadline (and thus may create a
    /// tokio timer when waiting).
    pub(crate) fn has_deadline(&self) -> bool {
        self.deadline.is_some()
    }

    /// Polls for the next chunk, driving the connection in place.
    pub(crate) fn poll_chunk(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<Option<Bytes>, Failure>> {
        use std::future::Future;
        use std::task::Poll;
        if self.passed() {
            return Poll::Ready(Err(failed("too large", "")));
        }
        loop {
            let next_data: Option<Bytes> = match &mut self.source {
                BodySource::Done => return Poll::Ready(Ok(None)),
                BodySource::Ready(_) => {
                    let BodySource::Ready(ready) = std::mem::replace(&mut self.source, BodySource::Done) else {
                        unreachable!();
                    };
                    let bytes = ready.into_bytes();
                    if bytes.is_empty() { None } else { Some(bytes) }
                }
                BodySource::Http1(body) => match body.poll_chunk(cx) {
                    Poll::Ready(Ok(Some(bytes))) => {
                        self.sleep = None;
                        Some(bytes)
                    }
                    Poll::Ready(Ok(None)) => {
                        self.sleep = None;
                        self.source = BodySource::Done;
                        None
                    }
                    Poll::Ready(Err(error)) => {
                        self.sleep = None;
                        self.source = BodySource::Done;
                        return Poll::Ready(Err(error));
                    }
                    Poll::Pending => {
                        if let Some(deadline) = self.deadline {
                            let sleep =
                                self.sleep.get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline.into())));
                            if sleep.as_mut().poll(cx).is_ready() {
                                self.sleep = None;
                                return Poll::Ready(Err(failed("timeout", "the request timed out")));
                            }
                        }
                        return Poll::Pending;
                    }
                },
                BodySource::Http2 { incoming, connection } => {
                    connection.run();
                    let frame = match hyper::body::Body::poll_frame(std::pin::Pin::new(incoming), cx) {
                        Poll::Ready(frame) => {
                            self.sleep = None;
                            frame
                        }
                        Poll::Pending => {
                            if let Some(deadline) = self.deadline {
                                let sleep = self
                                    .sleep
                                    .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(deadline.into())));
                                if sleep.as_mut().poll(cx).is_ready() {
                                    self.sleep = None;
                                    return Poll::Ready(Err(failed("timeout", "the request timed out")));
                                }
                            }
                            return Poll::Pending;
                        }
                    };
                    match frame {
                        Some(Ok(frame)) => match frame.into_data() {
                            Ok(data) => Some(data),
                            // Trailers.
                            Err(_) => continue,
                        },
                        Some(Err(error)) => {
                            self.source = BodySource::Done;
                            return Poll::Ready(Err(failed("body", with_causes(&error))));
                        }
                        None => {
                            self.source = BodySource::Done;
                            None
                        }
                    }
                }
            };
            let Some(data) = next_data else {
                let Some(decoder) = self.decoder.take() else { return Poll::Ready(Ok(None)) };
                let rest = match decoder.finish(self.room) {
                    Ok(rest) => rest,
                    Err(f) => return Poll::Ready(Err(self.refused(f))),
                };
                return Poll::Ready(match self.counted(rest) {
                    Ok(rest) => Ok(Some(rest).filter(|rest| !rest.is_empty())),
                    Err(e) => Err(e),
                });
            };
            let room = self.room;
            let decoded = match &mut self.decoder {
                None => return Poll::Ready(self.counted(data).map(Some)),
                Some(decoder) => decoder.feed(&data, room),
            };
            let decoded = match decoded {
                Ok(decoded) => decoded,
                Err(f) => return Poll::Ready(Err(self.refused(f))),
            };
            if !decoded.is_empty() {
                return Poll::Ready(self.counted(decoded).map(Some));
            }
        }
    }

    /// The next chunk, or none once it ended.
    pub(crate) async fn chunk(&mut self) -> Result<Option<Bytes>, Failure> {
        std::future::poll_fn(|cx| self.poll_chunk(cx)).await
    }

    /// How many bytes of the body are still to come, when its length is
    /// known and nothing decodes it.
    pub(crate) fn left(&self) -> Option<usize> {
        if self.decoder.is_some() {
            return None;
        }
        match &self.source {
            BodySource::Done => Some(0),
            BodySource::Ready(ready) => Some(ready.len()),
            BodySource::Http1(body) => match body.state {
                H1BodyState::Length(rem) => usize::try_from(rem).ok(),
                H1BodyState::Done => Some(0),
                H1BodyState::Chunked(_) | H1BodyState::UntilEof => None,
            },
            BodySource::Http2 { incoming, .. } => usize::try_from(hyper::body::Body::size_hint(incoming).exact()?).ok(),
        }
    }

    /// The whole body.
    pub(crate) async fn all(mut self) -> Result<Bytes, Failure> {
        let Some(first) = self.chunk().await? else { return Ok(Bytes::new()) };
        let Some(second) = self.chunk().await? else { return Ok(first) };
        let mut whole = bytes::BytesMut::with_capacity(first.len() + second.len());
        whole.extend_from_slice(&first);
        whole.extend_from_slice(&second);
        while let Some(chunk) = self.chunk().await? {
            whole.extend_from_slice(&chunk);
        }
        Ok(whole.freeze())
    }
}

/// What decodes a body of `Content-Encoding` gzip, deflate (zlib's, as
/// HTTP means it) or br, as the bytes come.
enum Decoder {
    Gzip(flate2::write::GzDecoder<Bounded>),
    Zlib(flate2::write::ZlibDecoder<Bounded>),
    Brotli(Box<brotli_decompressor::DecompressorWriter<Bounded>>),
}

/// What a decoder writes to: no more than `room` bytes, so that a few
/// bytes that decode to gigabytes stop at the bound inside the decoding.
#[derive(Default)]
struct Bounded {
    out: Vec<u8>,
    room: u64,
}

impl std::io::Write for Bounded {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if (self.out.len() + data.len()) as u64 > self.room {
            return Err(crate::too_large(self.room as usize));
        }
        self.out.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A decoder's error: `too large` past its room, else the body's.
fn undecoded(error: std::io::Error) -> Failure {
    match crate::limit_passed(&error) {
        Some(_) => failed("too large", String::new()),
        None => failed("body", format!("the body does not decode: {error}")),
    }
}

impl Decoder {
    fn of(encoding: &str) -> Option<Decoder> {
        let sink = Bounded::default;
        match encoding.trim().to_ascii_lowercase().as_str() {
            "gzip" | "x-gzip" => Some(Decoder::Gzip(flate2::write::GzDecoder::new(sink()))),
            "deflate" => Some(Decoder::Zlib(flate2::write::ZlibDecoder::new(sink()))),
            "br" => Some(Decoder::Brotli(Box::new(brotli_decompressor::DecompressorWriter::new(sink(), 4096)))),
            _ => None,
        }
    }

    /// What the bytes so far decode to, up to `room` bytes.
    fn feed(&mut self, data: &[u8], room: u64) -> Result<Bytes, Failure> {
        use std::io::Write;
        let output = match self {
            Decoder::Gzip(decoder) => {
                decoder.get_mut().room = room;
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(undecoded)?;
                decoder.get_mut()
            }
            Decoder::Zlib(decoder) => {
                decoder.get_mut().room = room;
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(undecoded)?;
                decoder.get_mut()
            }
            Decoder::Brotli(decoder) => {
                decoder.get_mut().room = room;
                decoder.write_all(data).and_then(|()| decoder.flush()).map_err(undecoded)?;
                decoder.get_mut()
            }
        };
        Ok(Bytes::from(std::mem::take(&mut output.out)))
    }

    /// What is left once the body ended, up to `room` bytes, which must end
    /// the encoding too.
    fn finish(mut self, room: u64) -> Result<Bytes, Failure> {
        match &mut self {
            Decoder::Gzip(decoder) => decoder.get_mut().room = room,
            Decoder::Zlib(decoder) => decoder.get_mut().room = room,
            Decoder::Brotli(decoder) => decoder.get_mut().room = room,
        }
        let rest = match self {
            Decoder::Gzip(decoder) => decoder.finish().map_err(undecoded)?,
            Decoder::Zlib(decoder) => decoder.finish().map_err(undecoded)?,
            Decoder::Brotli(decoder) => {
                decoder.into_inner().map_err(|_| failed("body", "the body ends before its brotli does"))?
            }
        };
        Ok(Bytes::from(rest.out))
    }
}

/// A chunk of a body that is read as it is sent.
type Chunks = std::pin::Pin<Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send>>;

/// A body read from chunks as it is sent.
pub(crate) fn streamed(chunks: impl futures_util::Stream<Item = std::io::Result<Bytes>> + Send + 'static) -> Outgoing {
    use futures_util::TryStreamExt;
    Outgoing::Stream(http_body_util::StreamBody::new(chunks.map_ok(hyper::body::Frame::data)).boxed_unsync())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_http_request(io: &mut tokio::io::DuplexStream) -> String {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            let n = io.read(&mut tmp).await.unwrap();
            assert!(n > 0, "unexpected EOF while reading request");
            buf.extend_from_slice(&tmp[..n]);
            if find_header_end(&buf).is_some() {
                return String::from_utf8(buf).unwrap();
            }
        }
    }

    #[test]
    fn h1_handles_1xx_chunked_head_204_and_eof_bodies() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (client_io, mut server_io) = tokio::io::duplex(4096);
            let mut h1 = Http1::new(Box::new(client_io));
            let url = url::Url::parse("http://example.test/one").unwrap();
            let route = RouteRef { secure: false, host: "example.test", port: 80, via: ViaRef::Direct };

            // 1. Informational 100 + 103 followed by 200 with Content-Length.
            let server = tokio::spawn(async move {
                let req1 = read_http_request(&mut server_io).await;
                assert!(req1.starts_with("GET /one HTTP/1.1\r\n"));
                server_io
                    .write_all(
                        b"HTTP/1.1 100 Continue\r\n\r\n\
                          HTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\n\
                          HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello",
                    )
                    .await
                    .unwrap();

                // 2. HEAD with Content-Length: 9999 must not consume body bytes.
                let req2 = read_http_request(&mut server_io).await;
                assert!(req2.starts_with("HEAD /one HTTP/1.1\r\n"));
                server_io.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 9999\r\n\r\n").await.unwrap();

                // 3. 204 No Content must not consume body bytes.
                let req3 = read_http_request(&mut server_io).await;
                assert!(req3.starts_with("DELETE /one HTTP/1.1\r\n"));
                server_io.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();

                // 4. Chunked response with chunk extension, split writes, and trailer.
                let req4 = read_http_request(&mut server_io).await;
                assert!(req4.starts_with("GET /one HTTP/1.1\r\n"));
                server_io
                    .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=1\r\nhello\r\n")
                    .await
                    .unwrap();
                server_io.flush().await.unwrap();
                server_io.write_all(b"6\r\n world\r\n0\r\nX-Trailer: ok\r\n\r\n").await.unwrap();

                // 5. Close-delimited response (no Content-Length / Transfer-Encoding).
                let req5 = read_http_request(&mut server_io).await;
                assert!(req5.starts_with("GET /one HTTP/1.1\r\n"));
                server_io.write_all(b"HTTP/1.1 200 OK\r\n\r\nuntil-eof").await.unwrap();
                server_io.shutdown().await.unwrap();
            });

            let head1 = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::GET,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await
                .ok()
                .unwrap();
            assert_eq!(head1.status, hyper::StatusCode::OK);
            assert!(head1.keep_alive);
            assert!(matches!(head1.framing, H1Framing::Length(5)));
            assert_eq!(&h1.unread()[..5], b"hello");
            h1.consume(5);

            let head2 = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::HEAD,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await
                .ok()
                .unwrap();
            assert!(matches!(head2.framing, H1Framing::Empty));
            assert!(head2.keep_alive);

            let head3 = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::DELETE,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await
                .ok()
                .unwrap();
            assert_eq!(head3.status, hyper::StatusCode::NO_CONTENT);
            assert!(matches!(head3.framing, H1Framing::Empty));
            assert!(head3.keep_alive);

            let head4 = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::GET,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await
                .ok()
                .unwrap();
            assert!(matches!(head4.framing, H1Framing::Chunked));
            assert!(head4.keep_alive);
            let pool: Arc<IdlePool> = Arc::default();
            let mut body4 = Http1Body {
                conn: Some(h1),
                state: H1BodyState::Chunked(ChunkedState::SizeLine),
                return_to: Some((Arc::downgrade(&pool), route.to_owned())),
            };
            let mut collected = Vec::new();
            while let Some(chunk) = std::future::poll_fn(|cx| body4.poll_chunk(cx)).await.unwrap() {
                collected.extend_from_slice(&chunk);
            }
            assert_eq!(collected, b"hello world");
            // Connection was returned to the pool after trailers finished.
            let mut h1 = {
                let mut guard = pool.lock().unwrap();
                let bucket = guard.values_mut().next().unwrap();
                let Sender::One(h1) = bucket[0].1.pop().unwrap() else { panic!("expected Http1") };
                h1
            };

            let head5 = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::GET,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await
                .ok()
                .unwrap();
            assert!(matches!(head5.framing, H1Framing::UntilEof));
            assert!(!head5.keep_alive);
            let mut body5 = Http1Body { conn: Some(*h1), state: H1BodyState::UntilEof, return_to: None };
            let mut eof_bytes = Vec::new();
            while let Some(chunk) = std::future::poll_fn(|cx| body5.poll_chunk(cx)).await.unwrap() {
                eof_bytes.extend_from_slice(&chunk);
            }
            assert_eq!(eof_bytes, b"until-eof");
            server.await.unwrap();
        });
    }

    #[test]
    fn h1_rejects_conflicting_content_length() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            let (client_io, mut server_io) = tokio::io::duplex(4096);
            let mut h1 = Http1::new(Box::new(client_io));
            let url = url::Url::parse("http://example.test/bad").unwrap();
            let route = RouteRef { secure: false, host: "example.test", port: 80, via: ViaRef::Direct };

            let server = tokio::spawn(async move {
                let _ = read_http_request(&mut server_io).await;
                server_io
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\nhello")
                    .await
                    .unwrap();
            });
            let result = h1
                .send_request_and_read_head(H1Request {
                    method: &hyper::Method::GET,
                    url: &url,
                    route: &route,
                    headers: &[],
                    outgoing: Outgoing::Empty,
                    length: None,
                    kind: None,
                    cookies: &None,
                    decodes: true,
                })
                .await;
            assert!(matches!(result, Err(H1Error::Protocol(_))));
            server.await.unwrap();
        });
    }
}
