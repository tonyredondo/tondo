# std.acme

the certificates of an ACME authority (RFC 8555), as Let's Encrypt, which a server obtains and renews by itself while it runs. The authority checks each name with TLS-ALPN-01 (RFC 8737), which the server answers on its own port, so nothing else has to be reachable.

```tondo
import std.acme
import std.http

var app = http.App()
app.get("/", (_) { http.Reply.text("hello") })
app.listen(port: 443, tls: acme.tls(["example.com"], email: "me@example.com"))?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.acme` shows the same in a terminal.

- [Functions](#functions): [`LetsEncrypt`](#letsencrypt), [`LetsEncryptStaging`](#letsencryptstaging), [`tls`](#tls)
- [AcmeError](#acmeerror)

### Functions

#### LetsEncrypt

```tondo
pub const LetsEncrypt: String = "https://acme-v02.api.letsencrypt.org/directory"
```

Let's Encrypt's directory.

#### LetsEncryptStaging

```tondo
pub const LetsEncryptStaging: String = "https://acme-staging-v02.api.letsencrypt.org/directory"
```

Let's Encrypt's directory for tests, whose certificates no browser
trusts and whose limits are higher.

#### tls

```tondo
pub fn tls(
    domains: Array[String],
    email: String,
    directory: String = LetsEncrypt,
    cache: String? = none,
    trust: String? = none,
): net.Tls
```

A server's TLS whose certificate for `domains`, the first its subject,
comes from the ACME authority of `directory` and renews itself when a
third of its life is left: `app.listen(port: 443, tls: acme.tls(["example.com"],
email: "me@example.com"))`. `email` is who the authority writes to about
them; `cache` keeps the account and the certificates, by default under
the user's cache directory; `trust` is the authority of the directory's
own certificate, for a test server. Until the first certificate arrives
handshakes fail; an attempt that fails is logged and tried again.

### AcmeError

```tondo
pub type AcmeError = {
    message: String
}
```

Why a certificate could not be obtained.

```tondo
import std.acme
import std.console
import std.http

var app = http.App()
// A certificate the authority does not give fails the server's start.
match app.start(port: 0, tls: acme.tls(["example.com"], email: "me@example.com")) {
    ok(server) => server.stop()
    err(error) => console.println("{error}")
}
```

Implements `Display`, `reflect.Reflect`.
