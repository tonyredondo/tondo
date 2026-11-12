# std.url

URLs and relative references as RFC 3986 defines them: their parts, resolving one from another, and the query of a form.

```tondo
import std.console
import std.url

let address = url.parse("https://example.com:8443/docs/a%20b?page=2")?
console.println("{address.host} {address.port} {address.path} {address.param("page")}")
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.url` shows the same in a terminal.

- [Functions](#functions): [`parse`](#parse), [`encodeQuery`](#encodequery)
- [Url](#url): [`Url.isAbsolute`](#urlisabsolute), [`Url.resolve`](#urlresolve), [`Url.param`](#urlparam), [`Url.params`](#urlparams), [`Url.escapedPath`](#urlescapedpath)
- [UrlError](#urlerror)

### Functions

#### parse

```tondo
pub fn parse(text: String): Url ! UrlError
```

The URL or relative reference `text` writes: `https://example.com/a`,
`/docs`, `../a.png`, `?page=2` or `#top`.

#### encodeQuery

```tondo
pub fn encodeQuery(values: Map[String, String]): String
```

A query of the names and values, in the map's order, as a form writes
them: `q=a+b&page=2`.

### Url

```tondo
pub type Url = {
    /// `https`; empty in a relative reference.
    scheme: String
    user: String
    password: String?
    /// A name or an address, without brackets for IPv6; empty without one.
    host: String
    port: Int?
    /// Decoded: `/docs/a b` for `/docs/a%20b`.
    path: String
    /// As it is written, without `?`.
    query: String
    /// Decoded, without `#`.
    fragment: String
    // private fields
}
```

A URL, `https://ada@example.com:8443/docs/a?q=1#intro`, or a relative
reference, `../a.png`. `Display` writes it back, encoding what the path
and the fragment need, so one changed with `with` is a valid URL.

```tondo
import std.console
import std.url

let base = url.parse("https://example.com/docs/guide/?lang=en")?
let next = base.resolve("../api?page=2&page=3")?
console.println("{base.isAbsolute()} {base.param("lang")} {next.path} {next.params("page")}")
```

Implements `Display`, `reflect.Reflect`.

#### Url.isAbsolute

```tondo
pub fn Url.isAbsolute(self): Bool
```

Whether it has a scheme.

#### Url.resolve

```tondo
pub fn Url.resolve(self, reference: String): Url ! UrlError
```

The URL a reference points to, seen from this one, as a browser resolves
a link (RFC 3986, 5.2): with its `.` and `..` removed.

#### Url.param

```tondo
pub fn Url.param(self, name: String): String?
```

The first value of `name` in the query, read as a form's: `+` is a
space.

#### Url.params

```tondo
pub fn Url.params(self, name: String): Array[String]
```

Every value of `name` in the query, in order.

#### Url.escapedPath

```tondo
pub fn Url.escapedPath(self): String
```

The path as the URL writes it: as `parse` read it, `/a%2Fb`, while
`path` is what that decodes to, and otherwise `path` encoded.

### UrlError

```tondo
pub type UrlError = {
    message: String
    offset: Int
}
```

Why a text is not a URL: what to fix, and where, in characters.

```tondo
import std.console
import std.url

match url.parse("http://[::1/") {
    ok(address) => console.println(address.host)
    err(error) => console.println("at {error.offset}: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.
