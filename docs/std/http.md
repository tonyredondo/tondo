# std.http

An HTTP server and an HTTP client, for HTTP/1.1 and HTTP/2 over TLS, with
bodies that stream both ways, forms and uploads, Server-Sent Events and
WebSockets. A service needs nothing else for the web: routing, JSON in and
out, validation, errors as RFC 9457 problems, OpenAPI, static files,
cookies and CORS are here. A server answers each request in a task of its
own, on every core, and the handlers are ordinary functions that wait.

- [A server and a client](#a-server-and-a-client)
- [Typed handlers, bodies and problems](#typed-handlers-bodies-and-problems)
- [Around every request](#around-every-request)
- [What to reach for](#what-to-reach-for)

## A server and a client

```tondo
import std.console
import std.http

type Greeting = { message: String, length: Int }

fn main(): !http.HttpError {
    var app = http.App()
    app.get("/hello/:name", (request) {
        let name = request.param("name")
        http.Reply.json(Greeting { message: "Hello, {name}!", length: name.length() })
    })
    let server = app.start()?                   // on a free port; `listen(port: 8080)` serves until stopped
    defer server.stop()

    let greeting: Greeting = http.get("{server.url()}/hello/Ada")?.json()?
    console.println("{greeting.message} ({greeting.length})")
    match http.get("{server.url()}/nothing") {
        err(http.HttpError.Status(response)) => console.println("{response.status()}")
        _ => console.println("unexpected")
    }
}
```

```text output
Hello, Ada! (3)
404
```

- A route is a method, a path with `:parameters`, and a handler that takes
  the [`Request`](#request) and gives a [`Reply`](#reply), or fails with a
  [`Problem`](#problem).
- The client fails with [`HttpError.Status`](#httperror) for a status of 400
  or more, so a reply that is read is one that succeeded.
- `app.start()` serves in the background and gives a [`Server`](#server) to
  stop, which is how tests use it; `app.listen()` serves until the process
  is asked to end, and then drains the requests in flight.

## Typed handlers, bodies and problems

A handler can take its parameters as arguments. They are read from the
path and the query by their names, a value that does not fit is a 400, and
what the handler returns is written as JSON: `none` is a 404.

```tondo
import std.console
import std.http
import std.validate

type Task = { id: Int, title: String }

type NewTask = {
    @validate.Length(min: 1, max: 80)
    title: String
}

fn find(id: Int): Task? {
    if id == 1 { Task { id, title: "write the manual" } } else { none }
}

fn main(): !http.HttpError {
    var app = http.App()
    app.get("/tasks/:id", (id: Int) { find(id) })
    app.post("/tasks", (request) {
        let input = request.json[NewTask]()?    // checked against the type and its rules
        http.Reply.json(Task { id: 2, title: input.title }, status: 201)
    })
    let server = app.start()?
    defer server.stop()
    let client = http.Client(baseUrl: server.url())

    console.println(client.get("/tasks/1")?.text()?)
    console.println("{client.post("/tasks", body: http.Body.json(NewTask { title: "test it" }))?.status()}")
    for request in ["/tasks/7", "/tasks/seven"] {
        match client.get(request) {
            err(http.HttpError.Status(response)) => console.println("{response.status()}")
            _ => console.println("unexpected")
        }
    }
    match client.post("/tasks", body: http.Body.json(NewTask { title: "" })) {
        err(http.HttpError.Status(response)) => console.println("{response.status()} {response.text()?}")
        _ => console.println("unexpected")
    }
}
```

```text output
{"id":1,"title":"write the manual"}
201
404
400
422 {"status":422,"detail":"the value breaks the rules of its type: title: must have at least 1 character","errors":[{"path":"title","message":"must have at least 1 character"}]}
```

A handler fails with `fail http.Problem.notFound("no such task")`, or with
`?` on anything that is a `Problem`. A status of 500 or more keeps its
message for the log and answers `internal server error`.

## Around every request

Middleware is a function that takes the request and the rest of the chain.
It is where logging, authentication and metrics go, once:

```tondo
import std.console
import std.http

fn main(): !http.HttpError {
    var app = http.App()
    app.use((request, next) {
        if request.header("Authorization") != "Bearer secret" {
            fail http.Problem.unauthorized("a key is needed")
        }
        next(request)?.header("X-Served-By", "tondo")
    })
    app.get("/", (_) { http.Reply.text("hello") })
    let server = app.start()?
    defer server.stop()

    let answer = http.get(server.url(), headers: ["Authorization": "Bearer secret"])?
    console.println("{answer.text()?} {answer.header("X-Served-By")}")
    match http.get(server.url()) {
        err(http.HttpError.Status(response)) => console.println("{response.status()}")
        _ => console.println("unexpected")
    }
}
```

```text output
hello some(tondo)
401
```

A `context` set in a middleware with `with` holds for the handler and for
every function and task under it, which is how a request's id reaches each
line of its log ([Contexts](../language/14-contexts.md)).

## What to reach for

| To | Use |
| --- | --- |
| Answer JSON, text, HTML, bytes or a file | [`Reply.json`](#replyjson), [`Reply.text`](#replytext), [`Reply.html`](#replyhtml), [`Reply.bytes`](#replybytes), [`Reply.file`](#replyfile) |
| Stream an answer, or send events | [`Reply.stream`](#replystream), [`Reply.events`](#replyevents) |
| Read what was sent | [`Request.json`](#requestjson), [`Request.form`](#requestform), [`Request.uploads`](#requestuploads), [`Request.queryAs`](#requestqueryas) |
| Serve a directory, or files kept in the executable | [`App.files`](#appfiles) |
| Talk over a WebSocket | [`App.websocket`](#appwebsocket), [`WebSocket.connect`](#websocketconnect) |
| Describe the API | [`App.openApi`](#appopenapi) |
| Let a page of another origin call it | [`App.cors`](#appcors) |
| Serve gRPC beside the routes | [`App.grpc`](#appgrpc), with [`std.grpc`](grpc.md) |
| Call another service | [`get`](#get), [`post`](#post), or a [`Client`](#client) with a base URL, headers and a timeout |
| Send a form, a file or a stream | [`Body.form`](#bodyform), [`Body.multipart`](#bodymultipart), [`Body.stream`](#bodystream) |
| Read an answer as it arrives | [`Response.lines`](#responselines), [`Response.events`](#responseevents), [`Response.body`](#responsebody) |
| Wrap every request a client makes | [`Client.use`](#clientuse) |

The [`todo-api`](../../examples/todo-api) example is a whole service, the
[`chat`](../../examples/chat) one uses WebSockets, and
[`linkcheck`](../../examples/linkcheck) the client.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.http` shows the same in a terminal.

- [Functions](#functions): [`get`](#get), [`head`](#head), [`delete`](#delete), [`post`](#post), [`put`](#put), [`patch`](#patch), [`mediaType`](#mediatype), [`proxy`](#proxy)
- [HttpError](#httperror)
- [Body](#body): [`Body.text`](#bodytext), [`Body.bytes`](#bodybytes), [`Body.json`](#bodyjson), [`Body.form`](#bodyform), [`Body.multipart`](#bodymultipart), [`Body.stream`](#bodystream)
- [Part](#part): [`Part.field`](#partfield), [`Part.bytes`](#partbytes), [`Part.file`](#partfile), [`Part.stream`](#partstream)
- [Response](#response): [`Response.status`](#responsestatus), [`Response.reason`](#responsereason), [`Response.url`](#responseurl), [`Response.version`](#responseversion), [`Response.header`](#responseheader), [`Response.headerValues`](#responseheadervalues), [`Response.headers`](#responseheaders), [`Response.bytes`](#responsebytes), [`Response.text`](#responsetext), [`Response.json`](#responsejson), [`Response.body`](#responsebody), [`Response.lines`](#responselines), [`Response.events`](#responseevents), [`Response.copyTo`](#responsecopyto), [`Response.cookie`](#responsecookie), [`Response.cookies`](#responsecookies)
- [Lines](#lines)
- [Event](#event)
- [Events](#events)
- [Client](#client): [`Client`](#client-1), [`Client.get`](#clientget), [`Client.head`](#clienthead), [`Client.delete`](#clientdelete), [`Client.post`](#clientpost), [`Client.put`](#clientput), [`Client.patch`](#clientpatch), [`Client.request`](#clientrequest), [`Client.use`](#clientuse)
- [ClientRequest](#clientrequest-1)
- [Message](#message)
- [WebSocket](#websocket): [`WebSocket.connect`](#websocketconnect), [`WebSocket.send`](#websocketsend), [`WebSocket.sendText`](#websocketsendtext), [`WebSocket.sendBytes`](#websocketsendbytes), [`WebSocket.receive`](#websocketreceive), [`WebSocket.close`](#websocketclose)
- [Problem](#problem): [`Problem`](#problem-1), [`Problem.badRequest`](#problembadrequest), [`Problem.unauthorized`](#problemunauthorized), [`Problem.forbidden`](#problemforbidden), [`Problem.notFound`](#problemnotfound), [`Problem.invalid`](#probleminvalid), [`Problem.conflict`](#problemconflict), [`Problem.from`](#problemfrom)
- [Request](#request): [`Request.method`](#requestmethod), [`Request.route`](#requestroute), [`Request.path`](#requestpath), [`Request.rawQuery`](#requestrawquery), [`Request.host`](#requesthost), [`Request.scheme`](#requestscheme), [`Request.url`](#requesturl), [`Request.version`](#requestversion), [`Request.clientCertificate`](#requestclientcertificate), [`Request.remoteAddress`](#requestremoteaddress), [`Request.clientIp`](#requestclientip), [`Request.param`](#requestparam), [`Request.intParam`](#requestintparam), [`Request.query`](#requestquery), [`Request.queryValues`](#requestqueryvalues), [`Request.queryAs`](#requestqueryas), [`Request.formAs`](#requestformas), [`Request.requiredQuery`](#requestrequiredquery), [`Request.requiredIntQuery`](#requestrequiredintquery), [`Request.intQuery`](#requestintquery), [`Request.requiredBoolQuery`](#requestrequiredboolquery), [`Request.boolQuery`](#requestboolquery), [`Request.requiredFloatQuery`](#requestrequiredfloatquery), [`Request.floatQuery`](#requestfloatquery), [`Request.header`](#requestheader), [`Request.headerValues`](#requestheadervalues), [`Request.headers`](#requestheaders), [`Request.limitBody`](#requestlimitbody), [`Request.withBody`](#requestwithbody), [`Request.bytes`](#requestbytes), [`Request.text`](#requesttext), [`Request.json`](#requestjson), [`Request.form`](#requestform), [`Request.uploads`](#requestuploads), [`Request.body`](#requestbody), [`Request.saveTo`](#requestsaveto), [`Request.cookie`](#requestcookie), [`Request.cookies`](#requestcookies)
- [UploadedFile](#uploadedfile)
- [Upload](#upload): [`Upload.name`](#uploadname), [`Upload.fileName`](#uploadfilename), [`Upload.contentType`](#uploadcontenttype), [`Upload.bytes`](#uploadbytes), [`Upload.text`](#uploadtext), [`Upload.saveTo`](#uploadsaveto)
- [Uploads](#uploads)
- [Reply](#reply): [`Reply.text`](#replytext), [`Reply.html`](#replyhtml), [`Reply.json`](#replyjson), [`Reply.bytes`](#replybytes), [`Reply.stream`](#replystream), [`Reply.file`](#replyfile), [`Reply.events`](#replyevents), [`Reply.redirect`](#replyredirect), [`Reply.empty`](#replyempty), [`Reply.notFound`](#replynotfound), [`Reply.header`](#replyheader), [`Reply.addHeader`](#replyaddheader), [`Reply.withStatus`](#replywithstatus), [`Reply.body`](#replybody), [`Reply.withBody`](#replywithbody), [`Reply.problem`](#replyproblem), [`Reply.status`](#replystatus), [`Reply.headers`](#replyheaders), [`Reply.cookie`](#replycookie)
- [Trailers](#trailers): [`Trailers`](#trailers-1), [`Trailers.set`](#trailersset), [`Trailers.all`](#trailersall)
- [SameSite](#samesite)
- [App](#app): [`App.websocket`](#appwebsocket), [`App`](#app-1), [`App.get`](#appget), [`App.post`](#apppost), [`App.put`](#appput), [`App.patch`](#apppatch), [`App.delete`](#appdelete), [`App.route`](#approute), [`App.use`](#appuse), [`App.cors`](#appcors), [`App.mount`](#appmount), [`App.grpc`](#appgrpc), [`App.files`](#appfiles), [`App.uploads`](#appuploads), [`App.compress`](#appcompress), [`App.client`](#appclient), [`App.trustProxies`](#apptrustproxies), [`App.openApi`](#appopenapi), [`App.listen`](#applisten), [`App.start`](#appstart)
- [GrpcService](#grpcservice)
- [Saved](#saved)
- [Limits](#limits): [`Limits.standard`](#limitsstandard)
- [Server](#server): [`Server.address`](#serveraddress), [`Server.url`](#serverurl), [`Server.stop`](#serverstop)
- [Tls](#tls)
- [TlsVersion](#tlsversion)

### Functions

#### get

```tondo
pub fn get(
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `GET` on a connection of its own; a status of 400 or more is
`HttpError.Status`, with the response.

#### head

```tondo
pub fn head(
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `HEAD` on a connection of its own: the headers alone.

#### delete

```tondo
pub fn delete(
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `DELETE` on a connection of its own.

#### post

```tondo
pub fn post(
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `POST` with the body, on a connection of its own.

#### put

```tondo
pub fn put(
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `PUT` with the body, on a connection of its own.

#### patch

```tondo
pub fn patch(
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `PATCH` with the body, on a connection of its own.

#### mediaType

```tondo
pub fn mediaType(path: String): String
```

The media type a file's name says, as `text/css` for `site.css`, which
`App.files` sends; `application/octet-stream` when it says none.

#### proxy

```tondo
pub fn proxy(
    target: String,
    strip: String = "",
    client: Client? = none,
): fn(Request): Reply ! Problem
```

A handler that sends each request on to `target`, as a reverse proxy
does, and its response back: the method, the path and query, the headers
and the body, each body streamed as it arrives. The path loses `strip`
first; `X-Forwarded-For`, `X-Forwarded-Host` and `X-Forwarded-Proto` tell
the target who asked; and a target that does not answer is `502`.
`client` sends the requests, with its hooks, so what a proxy sends is
observed as any other request: `app.route("*", "/api/*", http.proxy(
"http://backend:8080", strip: "/api"))`.

### HttpError

```tondo
pub enum HttpError {
    /// A URL that does not parse, or whose scheme is not `http` or `https`.
    InvalidUrl(String)
    /// The name did not resolve, the server refused, or TLS failed.
    Connect(String)
    /// The server cannot take the address, as one that is in use.
    Listen(String)
    /// The request took longer than its timeout.
    Timeout
    /// The server answered with a status of 400 or more: the response, to
    /// read its body.
    Status(Response)
    /// The request's or the response's body failed, or is not what was
    /// asked for: not UTF-8, not the JSON of the type.
    Body(String)
    /// Anything else of HTTP: too many redirects, a broken connection.
    Protocol(String)
}
```

Why a request has no response a program can use.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/missing", (_) { fail http.Problem.notFound("no such page") })
match app.client().get("/missing") {
    ok(response) => console.println("{response.status()}")
    err(http.HttpError.Status(response)) => console.println("{response.status()}: {response.text()?}")
    err(http.HttpError.Timeout) => console.println("no answer in time")
    err(error) => console.println("{error}")
}
```

Implements `Display`, `reflect.Reflect`.

### Body

```tondo
pub type Body = {
    // private fields
}
```

What a request sends.

```tondo
import std.console
import std.http

var app = http.App()
app.post("/echo", (request) {
    http.Reply.text("{request.header("Content-Type") ?? ""}: {request.text()?}")
})
let client = app.client()
for body in [http.Body.text("hi"), http.Body.json(["a": 1]), http.Body.form(["q": "tea & cake"])] {
    console.println(client.post("/echo", body: some(body))?.text()?)
}
```

#### Body.text

```tondo
pub fn Body.text(text: String, contentType: String = "text/plain; charset=utf-8"): Body
```

A body of text, sent as UTF-8 with its content type.

#### Body.bytes

```tondo
pub fn Body.bytes(bytes: Array[Byte], contentType: String = "application/octet-stream"): Body
```

A body of bytes, sent with its content type.

#### Body.json

```tondo
pub fn Body.json[T: serialization.Encode](value: T): Body
```

The value as JSON (`std.json`).

#### Body.form

```tondo
pub fn Body.form(fields: Map[String, String]): Body
```

The fields as a URL-encoded form, as an HTML form posts them.

#### Body.multipart

```tondo
pub fn Body.multipart(parts: Array[Part]): Body
```

A `multipart/form-data` form: fields and files, as uploads send them.

#### Body.stream

```tondo
pub fn Body.stream(
    input: io.Reader,
    length: Int? = none,
    contentType: String = "application/octet-stream",
): Body
```

What a stream gives, read as it is sent; `length` is its size in bytes
when known. A stream is read once.

### Part

```tondo
pub type Part = {
    // private fields
}
```

A part of a multipart form.

```tondo
import std.console
import std.http

var app = http.App()
app.post("/albums", (request) {
    var parts: Array[String] = []
    for upload in request.uploads() {
        let part = upload?
        parts.push("{part.name()} ({part.fileName()}, {part.bytes()?.length()} bytes)")
    }
    http.Reply.text(parts.join(", "))
})
let form = http.Body.multipart([
    http.Part.field("title", "Holidays"),
    http.Part.bytes("photo", [1, 2, 3], fileName: "beach.jpg", contentType: "image/jpeg"),
])
console.println(app.client().post("/albums", body: some(form))?.text()?)
```

#### Part.field

```tondo
pub fn Part.field(name: String, value: String): Part
```

A field of a multipart form: its name and its text.

#### Part.bytes

```tondo
pub fn Part.bytes(
    name: String,
    bytes: Array[Byte],
    fileName: String,
    contentType: String = "",
): Part
```

A file's bytes, sent under `fileName`.

#### Part.file

```tondo
pub fn Part.file(name: String, path: String, contentType: String = ""): Part
```

A file on disk, read as it is sent, under its own name and with the
content type of its extension unless one is given.

#### Part.stream

```tondo
pub fn Part.stream(
    name: String,
    input: io.Reader,
    fileName: String,
    contentType: String = "",
    length: Int? = none,
): Part
```

What a stream gives, read as it is sent, under `fileName`.

### Response

```tondo
pub type Response = {
    // private fields
}
```

A response, whose head has arrived; its body arrives as it is read,
once, by `text`, `bytes`, `json`, `body`, `lines`, `events` or `copyTo`.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/", (_) { http.Reply.text("hello").header("X-Version", "2").cookie("session", "abc") })
let response = app.client().get("/")?
console.println("{response.status()} {response.reason()} {response.version()}")
console.println("{response.header("x-version")} {response.cookie("session")} {response.text()?}")
```

Implements `Display`.

#### Response.status

```tondo
pub fn Response.status(self): Int
```

The status code: `200`, `404`.

#### Response.reason

```tondo
pub fn Response.reason(self): String
```

The status's reason phrase: `OK`, `Not Found`.

#### Response.url

```tondo
pub fn Response.url(self): String
```

The URL that answered, after redirects.

#### Response.version

```tondo
pub fn Response.version(self): String
```

`HTTP/1.1` or `HTTP/2.0`.

#### Response.header

```tondo
pub fn Response.header(self, name: String): String?
```

The first value of a header, by its name in any case.

#### Response.headerValues

```tondo
pub fn Response.headerValues(self, name: String): Array[String]
```

Every value of a header, by its name in any case.

#### Response.headers

```tondo
pub fn Response.headers(self): Map[String, String]
```

The headers, lowercase, with the values of a repeated one joined by
`, `.

#### Response.bytes

```tondo
pub fn Response.bytes(self): Array[Byte] ! HttpError
```

The whole body, read as it arrives.

#### Response.text

```tondo
pub fn Response.text(self): String ! HttpError
```

The body as text, in the charset its content type names (`windows1252`).

#### Response.json

```tondo
pub fn Response.json[T: serialization.Decode](self): T ! HttpError
```

The body as the JSON of a `T` (`std.json`), decoded as it arrives.

#### Response.body

```tondo
pub fn Response.body(self): io.Reader
```

The body as a stream, read as the program asks for it.

#### Response.lines

```tondo
pub fn Response.lines(self): Lines
```

The body's lines, as they arrive, in the charset `text()` reads:
newline-delimited JSON, logs.

#### Response.events

```tondo
pub fn Response.events(self): Events
```

The body's Server-Sent Events, as they arrive, in UTF-8 as they are.

#### Response.copyTo

```tondo
pub fn Response.copyTo(self, output: io.Writer): Int ! HttpError
```

Writes the body to a stream as it arrives; how many bytes it wrote.

#### Response.cookie

```tondo
pub fn Response.cookie(self, name: String): String?
```

The value of a cookie the response sets, decoded as `Request.cookie`
does, or `none`.

#### Response.cookies

```tondo
pub fn Response.cookies(self): Map[String, String]
```

The cookies the response sets, by name, their values decoded.

### Lines

```tondo
pub type Lines = {
    // private fields
}
```

The lines of a body, as `Response.lines()` gives them.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/log", (_) { http.Reply.text("one\ntwo\nthree\n") })
// Each line as it arrives, without the whole body in memory.
for line in app.client().get("/log")?.lines() {
    console.println("> {line?}")
}
```

Implements `Iterator[String ! HttpError]`.

### Event

```tondo
pub type Event = {
    event: String
    data: String
    id: String?
}
```

A Server-Sent Event: its type, `message` unless the server names one,
its data, with a `\n` between data lines, and its id.

```tondo
import std.console
import std.http

fn tick(out: Sender[http.Event]) {
    for n in 1..=3 {
        out.send(http.Event { event: "tick", data: "{n}", id: some("{n}") })
    }
}

var app = http.App()
// Server-Sent Events: each event the channel gives, until it closes.
app.get("/ticks", (_) {
    let (sender, receiver) = channel[http.Event](4)
    spawn detached tick(sender)
    http.Reply.events(receiver)
})
for event in app.client().get("/ticks")?.events() {
    console.println(event?.data)
}
```

### Events

```tondo
pub type Events = {
    // private fields
}
```

The events of a body, as `Response.events()` gives them.

```tondo
import std.console
import std.http

let stream = http.get("https://stream.example.com/updates", headers: ["Accept": "text/event-stream"])?
for received in stream.events() {
    let event = received?
    console.println("{event.event} #{event.id ?? "-"}: {event.data}")
}
```

Implements `Iterator[Event ! HttpError]`.

### Client

```tondo
pub type Client = {
    // private fields
}
```

Sends requests and keeps their connections for the next ones, over
HTTP/2 when the server offers it. Its URLs may be relative to
`baseUrl`, and its headers go with every request. `trust` names a PEM
file of certificates it trusts besides the system's, such as a company's
or a development certificate authority.

```tondo
import std.console
import std.http

type Todo = { id: Int, title: String }

let web = http.Client(baseUrl: "https://api.example.com")
let todo = web.get("/todos/1")?.json[Todo]()?
let made = web.post("/todos", body: some(http.Body.json(todo with { id: 0 })))?
console.println("{todo.title} {made.status()}")
```

#### Client

```tondo
pub fn Client(
    baseUrl: String = "",
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
    connectTimeout: time.Duration? = none,
    redirects: Int = 10,
    trust: String? = none,
    socket: String? = none,
    cookies: Bool = false,
    identity: Tls? = none,
): Client
```

A client that keeps its connections for later requests: `baseUrl` goes
before each relative URL and `headers` with each request, whose own
headers win; `timeout` limits a request, its body included,
`connectTimeout` its connection; it follows up to `redirects` redirects,
and trusts the authorities of `trust` too, the path of a PEM file or its
text; it shows the certificate of `identity` to a server that asks for
one, as mutual TLS does. With `socket`,
each request goes through that Unix socket instead of the network, and
the URL gives only its path and `Host`. With `cookies`, it keeps the
cookies responses set and sends them with the next requests, as a
browser does.

#### Client.get

```tondo
pub fn Client.get(
    self,
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `GET`; a status of 400 or more is `HttpError.Status`, with the
response.

#### Client.head

```tondo
pub fn Client.head(
    self,
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `HEAD`: the headers alone.

#### Client.delete

```tondo
pub fn Client.delete(
    self,
    url: String,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `DELETE`.

#### Client.post

```tondo
pub fn Client.post(
    self,
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `POST` with the body.

#### Client.put

```tondo
pub fn Client.put(
    self,
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `PUT` with the body.

#### Client.patch

```tondo
pub fn Client.patch(
    self,
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

Sends a `PATCH` with the body.

#### Client.request

```tondo
pub fn Client.request(
    self,
    method: String,
    url: String,
    body: Body? = none,
    query: Map[String, String] = [:],
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): Response ! HttpError
```

A request with any method. It waits for the response's head; a status
of 400 or more is `HttpError.Status`.

#### Client.use

```tondo
pub fn Client.use(
    mut self,
    hook: fn(ClientRequest, fn(ClientRequest): Response ! HttpError): Response ! HttpError,
)
```

Adds a hook around each request of the client, as middleware is around
a server's routes: it gets the request and `next`, which sends it, and
may change it, time it, see its response or error, or answer without
sending it. The first added is the outermost; redirects happen inside
`next`.

### ClientRequest

```tondo
pub type ClientRequest = {
    method: String
    url: String
    headers: Map[String, String]
    body: Body?
}
```

A request a client sends, as its hooks see it (`Client.use`): its
method, its final URL, with the client's base and the query, its
headers, the client's and its own, and its body.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/me", (request) { http.Reply.text(request.header("Authorization") ?? "anonymous") })
var client = app.client()
// Every request of the client through this hook: logged and authorized.
client.use((request, next) {
    console.println("{request.method} {request.url}")
    var signed = request
    signed.headers["Authorization"] = "Bearer secret"
    next(signed)?
})
console.println(client.get("/me")?.text()?)
```

### Message

```tondo
pub enum Message {
    Text(String)
    Binary(Array[Byte])
}
```

A message of a WebSocket.

```tondo
import std.console
import std.http

fn echo(_: http.Request, socket: http.WebSocket): !http.HttpError {
    for {
        match socket.receive()? {
            some(http.Message.Text(text)) => socket.send(http.Message.Text("echo: {text}"))?
            some(http.Message.Binary(bytes)) => socket.send(http.Message.Binary(bytes.reversed()))?
            none => break
        }
    }
}

var app = http.App()
app.websocket("/echo", echo)
let server = app.start()?
let socket = http.WebSocket.connect("ws://{server.address()}/echo")?
socket.sendBytes([1, 2, 3])?
match socket.receive()? {
    some(http.Message.Binary(bytes)) => console.println("{bytes}")
    other => console.println("unexpected {other.isSome()}")
}
socket.close()?
server.stop()
```

Implements `Display`.

### WebSocket

```tondo
pub type WebSocket = {
    // private fields
}
```

A WebSocket connection (RFC 6455), over TLS for `wss://`. One task can
send while another receives; pings are answered on their own. Iterating
it receives messages until the connection closes.

```tondo
import std.console
import std.http

fn chat(_: http.Request, socket: http.WebSocket): !http.HttpError {
    socket.sendText("welcome")?
    for {
        match socket.receive()? {
            some(http.Message.Text(text)) => socket.sendText("you said {text}")?
            some(_) => {}
            none => break
        }
    }
}

var app = http.App()
app.websocket("/chat", chat)
let server = app.start()?
let socket = http.WebSocket.connect("ws://{server.address()}/chat")?
socket.sendText("hello")?
for _ in 0..2 {
    match socket.receive()? {
        some(http.Message.Text(text)) => console.println(text)
        _ => {}
    }
}
socket.close()?
server.stop()
```

Implements `Iterator[Message ! HttpError]`.

#### WebSocket.connect

```tondo
pub fn WebSocket.connect(
    url: String,
    headers: Map[String, String] = [:],
    timeout: time.Duration? = none,
): WebSocket ! HttpError
```

Opens a WebSocket (RFC 6455), over TLS for `wss://`; `timeout` limits
its handshake.

#### WebSocket.send

```tondo
pub fn WebSocket.send(self, message: Message): !HttpError
```

Sends a message, of text or bytes.

#### WebSocket.sendText

```tondo
pub fn WebSocket.sendText(self, text: String): !HttpError
```

Sends a text message.

#### WebSocket.sendBytes

```tondo
pub fn WebSocket.sendBytes(self, bytes: Array[Byte]): !HttpError
```

Sends a binary message.

#### WebSocket.receive

```tondo
pub fn WebSocket.receive(self): Message? ! HttpError
```

The next message, or `none` once the connection closed.

#### WebSocket.close

```tondo
pub fn WebSocket.close(self): !HttpError
```

Closes the connection, telling the server.

### Problem

```tondo
pub type Problem = {
    status: Int
    message: String
    /// The rules of its type a value breaks, which a `422` lists in
    /// `errors`.
    violations: Array[serialization.Violation]
}
```

Why a handler cannot answer: a status of 400 or more and a message for
the client, which receives it as `application/problem+json` (RFC 9457).
For a status of 500 or more the client sees only `internal server
error`, and the message goes to standard error.

```tondo
import std.console
import std.http

fn order(request: http.Request): http.Reply ! http.Problem {
    let id = request.intParam("id")?
    if id > 100 {
        fail http.Problem.notFound("there is no order {id}")
    }
    http.Reply.json(["id": id])
}

var app = http.App()
app.get("/orders/:id", order)
for path in ["/orders/7", "/orders/500", "/orders/x"] {
    match app.client().get(path) {
        ok(response) => console.println("{response.status()} {response.text()?}")
        err(http.HttpError.Status(response)) => console.println("{response.status()} {response.text()?}")
        err(error) => console.println("{error}")
    }
}
```

Implements `Display`.

#### Problem

```tondo
pub fn Problem(status: Int, message: String): Problem
```

A problem with an HTTP status and a message, which the server sends as
`application/problem+json`.

#### Problem.badRequest

```tondo
pub fn Problem.badRequest(message: String): Problem
```

A `400 Bad Request` problem.

#### Problem.unauthorized

```tondo
pub fn Problem.unauthorized(message: String = "authentication is required"): Problem
```

A `401 Unauthorized` problem.

#### Problem.forbidden

```tondo
pub fn Problem.forbidden(message: String = "this is not allowed"): Problem
```

A `403 Forbidden` problem.

#### Problem.notFound

```tondo
pub fn Problem.notFound(message: String = "not found"): Problem
```

A `404 Not Found` problem.

#### Problem.invalid

```tondo
pub fn Problem.invalid(violations: Array[serialization.Violation]): Problem
```

A `422 Unprocessable Content` problem: a value that breaks the rules of
its type, each violation in the `errors` of the reply.

#### Problem.conflict

```tondo
pub fn Problem.conflict(message: String): Problem
```

A `409 Conflict` problem.

#### Problem.from

```tondo
pub fn Problem.from[E: Display](error: E): Problem
```

A failure of the server's own, for `result.mapErr(http.Problem.from)?`.

### Request

```tondo
pub type Request = {
    // private fields
}
```

A request a server received.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/users/:id", (request) {
    let id = request.intParam("id")?
    let fields = request.query("fields") ?? "all"
    let route = request.route() ?? ""
    http.Reply.text("{request.method()} {route} {request.path()}: user {id}, fields {fields}")
})
console.println(app.client().get("/users/42", query: ["fields": "name"])?.text()?)
```

#### Request.method

```tondo
pub fn Request.method(self): String
```

The method: `GET`, `POST`.

#### Request.route

```tondo
pub fn Request.route(self): String?
```

The template of the route that answers the request, as it was added and
with the prefix of `mount`: `/users/:id` for `GET /users/42`; `none` when
no route does. The middleware sees it before `next`: it names the request
for a tracer or a metric without growing with each value of a parameter,
as the path does.

#### Request.path

```tondo
pub fn Request.path(self): String
```

The path, without the query.

#### Request.rawQuery

```tondo
pub fn Request.rawQuery(self): String
```

The query as it arrived, without `?` and not decoded.

#### Request.host

```tondo
pub fn Request.host(self): String
```

The host the request was for, with its port if it gave one: `Host`, or
`:authority` in HTTP/2. Behind a proxy of a network the app trusts
(`App.trustProxies`), the one `X-Forwarded-Host` gives.

#### Request.scheme

```tondo
pub fn Request.scheme(self): String
```

`https` when the server listens with TLS, and `http` if not. Behind a
proxy of a network the app trusts, the one `X-Forwarded-Proto` gives.

#### Request.url

```tondo
pub fn Request.url(self): url.Url
```

The URL the request was for: its scheme, host, path and query.

#### Request.version

```tondo
pub fn Request.version(self): String
```

The protocol: `HTTP/1.1` or `HTTP/2`.

#### Request.clientCertificate

```tondo
pub fn Request.clientCertificate(self): crypto.Certificate?
```

The certificate the client showed, when the server requires one of
its clients (`Tls.files(..., clients: "ca.pem")`): who it is.

#### Request.remoteAddress

```tondo
pub fn Request.remoteAddress(self): String
```

The client's address and port.

#### Request.clientIp

```tondo
pub fn Request.clientIp(self): String
```

The client's IP address, without its port. A connection from a proxy of
a network the app trusts (`App.trustProxies`) gives the address the
proxies wrote: the last of `X-Forwarded-For` that is not of those
networks, or `X-Real-Ip`, as Gin's `ClientIP`. Without trusted proxies
no header is believed, since any client can write one.

#### Request.param

```tondo
pub fn Request.param(self, name: String): String
```

A parameter of the route, decoded: `id` in `/users/:id`.

#### Request.intParam

```tondo
pub fn Request.intParam(self, name: String): Int ! Problem
```

A parameter of the route that must be an integer.

#### Request.query

```tondo
pub fn Request.query(self, name: String): String?
```

The first value of a query parameter, decoded.

#### Request.queryValues

```tondo
pub fn Request.queryValues(self, name: String): Array[String]
```

Every value of a query parameter, decoded.

#### Request.queryAs

```tondo
pub fn Request.queryAs[T: serialization.Decode](self): T ! Problem
```

The query string read into a `T`: each field from the parameter of its
name, without case, `_` or `-`, so `page_size` fills `pageSize`; a list
from a name that repeats or one value; each value as text, which any
scalar reads. A parameter that does not read is a `400` problem, and a
value that breaks the rules of its type a `422`.

#### Request.formAs

```tondo
pub fn Request.formAs[T: serialization.Decode](self): T ! Problem
```

The fields of a form body read into a `T`, as `queryAs` reads the
query: a URL-encoded one, or a multipart one, whose files go to fields
of type `UploadedFile`, in memory.

#### Request.requiredQuery

```tondo
pub fn Request.requiredQuery(self, name: String): String ! Problem
```

A query parameter that must be there.

#### Request.requiredIntQuery

```tondo
pub fn Request.requiredIntQuery(self, name: String): Int ! Problem
```

A query parameter that must be there and be an integer.

#### Request.intQuery

```tondo
pub fn Request.intQuery(self, name: String): Int? ! Problem
```

A query parameter that must be an integer when it is there.

#### Request.requiredBoolQuery

```tondo
pub fn Request.requiredBoolQuery(self, name: String): Bool ! Problem
```

A query parameter that must be there and be `true` or `false`.

#### Request.boolQuery

```tondo
pub fn Request.boolQuery(self, name: String): Bool? ! Problem
```

A query parameter that must be `true` or `false` when it is there.

#### Request.requiredFloatQuery

```tondo
pub fn Request.requiredFloatQuery(self, name: String): Float ! Problem
```

A query parameter that must be there and be a number.

#### Request.floatQuery

```tondo
pub fn Request.floatQuery(self, name: String): Float? ! Problem
```

A query parameter that must be a number when it is there.

#### Request.header

```tondo
pub fn Request.header(self, name: String): String?
```

The first value of a header, by its name in any case.

#### Request.headerValues

```tondo
pub fn Request.headerValues(self, name: String): Array[String]
```

Every value of a header, by its name in any case, in order. The headers
stay as the server read them until one is asked for.

#### Request.headers

```tondo
pub fn Request.headers(self): Map[String, String]
```

The headers, lowercase, with the values of a repeated one joined by
`, `.

#### Request.limitBody

```tondo
pub fn Request.limitBody(self, bytes: Int): Request
```

The request with a body of up to `bytes`, more or fewer than the
server's `Limits.body`, as a route of uploads needs: before the body is
read, by the handler or a middleware.

#### Request.withBody

```tondo
pub fn Request.withBody(self, body: Array[Byte]): Request
```

The request with `body` for its body: what a middleware that read the
body gives on, as `next(request.withBody(bytes))`, for the handler to
read again.

#### Request.bytes

```tondo
pub fn Request.bytes(self): Array[Byte] ! Problem
```

The whole body; one that cannot be read is a `400` problem, and one
larger than its limit `413`.

#### Request.text

```tondo
pub fn Request.text(self): String ! Problem
```

The body as text, in the charset its content type names, as
`Response.text()` reads it.

#### Request.json

```tondo
pub fn Request.json[T: serialization.Decode](self): T ! Problem
```

The body as the JSON of a `T` (`std.json`), decoded as it arrives; a
body that is not one is `400` with the reason.

```tondo
import std.http

type Signup = { name: String, email: String }

var app = http.App()
app.post("/signups", (request) {
    let signup = request.json[Signup]()?
    http.Reply.json(signup, status: 201)
})
```

#### Request.form

```tondo
pub fn Request.form(self): Map[String, String] ! Problem
```

The fields of a URL-encoded form body.

#### Request.uploads

```tondo
pub fn Request.uploads(self): Uploads
```

The parts of a `multipart/form-data` body, in order, as they arrive.

#### Request.body

```tondo
pub fn Request.body(self): io.Reader
```

The body as a stream, read as the program asks for it.

#### Request.saveTo

```tondo
pub fn Request.saveTo(self, path: String): Int ! Problem
```

Writes the body to a file as it arrives; how many bytes.

#### Request.cookie

```tondo
pub fn Request.cookie(self, name: String): String?
```

The value of the cookie the client sent under `name`, decoded with
`encoding.urlDecode`, or `none`.

#### Request.cookies

```tondo
pub fn Request.cookies(self): Map[String, String]
```

The cookies the client sent, by name, their values decoded.

### UploadedFile

```tondo
pub type UploadedFile = {
    name: String
    fileName: String
    contentType: String
    bytes: Array[Byte]
}
```

A file of a multipart form, in memory, as `Request.formAs` gives it to
a field of its type: its field's name, the file's name and content type
the client gave, and its bytes.

```tondo
import std.console
import std.http

pub type Avatar = { user: String, picture: http.UploadedFile }

var app = http.App()
app.post("/avatar", (request) {
    let form = request.formAs[Avatar]()?
    let picture = form.picture
    http.Reply.text("{form.user}: {picture.fileName}, {picture.bytes.length()} bytes of {picture.contentType}")
})
let body = http.Body.multipart([
    http.Part.field("user", "ada"),
    http.Part.bytes("picture", [137, 80, 78, 71], fileName: "ada.png", contentType: "image/png"),
])
console.println(app.client().post("/avatar", body: some(body))?.text()?)
```

Implements `serialization.Encode`, `serialization.Decode`.

### Upload

```tondo
pub type Upload = {
    // private fields
}
```

A part of a multipart body. Parts arrive in order: reading one skips
what the ones before left unread.

```tondo
import std.console
import std.fs
import std.http
import std.path

let directory = fs.createTemporaryDirectory()?
var app = http.App()
app.post("/documents", (request) {
    var saved: Array[String] = []
    for received in request.uploads() {
        let upload = received?
        if upload.fileName().isEmpty() {
            saved.push("{upload.name()}={upload.text()?}")
        } else {
            let size = upload.saveTo(path.join(directory, path.fileName(upload.fileName()) ?? "upload"))?
            saved.push("{upload.fileName()} ({upload.contentType()}, {size} bytes)")
        }
    }
    http.Reply.text(saved.join("; "))
})
let body = http.Body.multipart([
    http.Part.field("owner", "ada"),
    http.Part.bytes("file", "report".bytes(), fileName: "report.txt", contentType: "text/plain"),
])
console.println(app.client().post("/documents", body: some(body))?.text()?)
```

#### Upload.name

```tondo
pub fn Upload.name(self): String
```

The form field's name.

#### Upload.fileName

```tondo
pub fn Upload.fileName(self): String
```

The file's name the client gave, or an empty string for a field.

#### Upload.contentType

```tondo
pub fn Upload.contentType(self): String
```

The content type the client gave the file.

#### Upload.bytes

```tondo
pub fn Upload.bytes(self): Array[Byte] ! Problem
```

The whole file; one that cannot be read is a `400` problem.

#### Upload.text

```tondo
pub fn Upload.text(self): String ! Problem
```

The file as UTF-8 text; one that is not is a `400` problem.

#### Upload.saveTo

```tondo
pub fn Upload.saveTo(self, path: String): Int ! Problem
```

Writes the part to a file as it arrives; how many bytes.

### Uploads

```tondo
pub type Uploads = {
    // private fields
}
```

The parts of a multipart body, as `Request.uploads()` gives them.

```tondo
import std.console
import std.http

var app = http.App()
// The parts of a multipart body, read one at a time as they arrive.
app.post("/count", (request) {
    var files = 0
    var bytes = 0
    for received in request.uploads() {
        let upload = received?
        files += 1
        bytes += upload.bytes()?.length()
    }
    http.Reply.text("{files} parts, {bytes} bytes")
})
let body = http.Body.multipart([
    http.Part.bytes("a", [1, 2], fileName: "a.bin"),
    http.Part.bytes("b", [3, 4, 5], fileName: "b.bin"),
])
console.println(app.client().post("/count", body: some(body))?.text()?)
```

Implements `Iterator[Upload ! Problem]`.

### Reply

```tondo
pub type Reply = {
    // private fields
}
```

What a handler answers.

```tondo
import std.console
import std.http

var app = http.App()
app.get("/", (_) { http.Reply.html("<h1>Home</h1>") })
app.get("/old", (_) { http.Reply.redirect("/", status: 301) })
app.get("/data", (_) { http.Reply.json(["ok": true]).header("Cache-Control", "no-store") })
app.delete("/items/:id", (_) { http.Reply.empty() })
let client = app.client()
console.println(client.get("/old")?.text()?)
let data = client.get("/data")?
console.println("{data.text()?} {data.header("cache-control")}")
console.println("{client.delete("/items/1")?.status()}")
```

#### Reply.text

```tondo
pub fn Reply.text(text: String, status: Int = 200): Reply
```

A reply of text, as `text/plain; charset=utf-8`.

#### Reply.html

```tondo
pub fn Reply.html(html: String, status: Int = 200): Reply
```

A reply of HTML, as `text/html; charset=utf-8`.

#### Reply.json

```tondo
pub fn Reply.json[T: serialization.Encode](value: T, status: Int = 200): Reply
```

The value as JSON (`std.json`).

#### Reply.bytes

```tondo
pub fn Reply.bytes(
    bytes: Array[Byte],
    contentType: String = "application/octet-stream",
    status: Int = 200,
): Reply
```

A reply of bytes, with their content type.

#### Reply.stream

```tondo
pub fn Reply.stream(
    input: io.Reader,
    contentType: String = "application/octet-stream",
    status: Int = 200,
    trailers: Trailers? = none,
): Reply
```

What a stream gives, sent as it gives it, and after it the `trailers`
set by the time it ends.

#### Reply.file

```tondo
pub fn Reply.file(path: String, download: String? = none): Reply
```

A file, with the content type of its extension. It answers `Range`
requests with the part asked for, so downloads resume, and
`If-None-Match` and `If-Modified-Since` with `304`. With `download`,
the browser saves it under that name. A missing file is `404`.

#### Reply.events

```tondo
pub fn Reply.events(events: Receiver[Event]): Reply
```

Server-Sent Events: each event the channel receives, until it closes.

#### Reply.redirect

```tondo
pub fn Reply.redirect(url: String, status: Int = 302): Reply
```

A reply that sends the client to `url`: `302 Found` unless another
status.

#### Reply.empty

```tondo
pub fn Reply.empty(status: Int = 204): Reply
```

A reply without a body: `204 No Content` unless another status.

#### Reply.notFound

```tondo
pub fn Reply.notFound(): Reply
```

A `404 Not Found` reply, as a problem.

#### Reply.header

```tondo
pub fn Reply.header(self, name: String, value: String): Reply
```

The same reply with the header `name` set to `value`, in place of the
values it had, whatever their case.

#### Reply.addHeader

```tondo
pub fn Reply.addHeader(self, name: String, value: String): Reply
```

The same reply with one more value of the header `name`, after the ones
it has: what `Set-Cookie`, `Vary` and `Link` take.

#### Reply.withStatus

```tondo
pub fn Reply.withStatus(self, status: Int): Reply
```

The same reply with another status code.

#### Reply.body

```tondo
pub fn Reply.body(self): Array[Byte]?
```

The body, when the reply holds it: that of text, HTML, JSON, bytes, a
redirect or a problem; `none` for a stream, a file, events or a
WebSocket, which are sent as they come.

#### Reply.withBody

```tondo
pub fn Reply.withBody(self, bytes: Array[Byte]): Reply
```

The same reply with this body in place of the one it had; its status
and headers stay, its content type too.

#### Reply.problem

```tondo
pub fn Reply.problem(problem: Problem): Reply
```

The reply the server gives for a problem, `application/problem+json`
(RFC 9457): for a middleware that adds its headers to a handler's
failure, `Reply.problem(problem).header(...)`.

#### Reply.status

```tondo
pub fn Reply.status(self): Int
```

The reply's status code.

#### Reply.headers

```tondo
pub fn Reply.headers(self): Map[String, String]
```

The headers set so far, lowercase, with the values of a repeated one
joined by `, `, as a middleware reads them.

#### Reply.cookie

```tondo
pub fn Reply.cookie(
    self,
    name: String,
    value: String,
    maxAge: time.Duration? = none,
    path: String = "/",
    domain: String = "",
    secure: Bool = false,
    httpOnly: Bool = true,
    sameSite: SameSite = SameSite.Lax,
): Reply
```

The reply with a cookie, its value written with `encoding.urlEncode` so
any text fits. A `maxAge` of zero removes it, and without one it lasts
the browser's session; `httpOnly` hides it from the page's JavaScript and
`secure` sends it over HTTPS only.

### Trailers

```tondo
pub type Trailers = {
    // private fields
}
```

The headers that follow the last bytes of a streamed reply, as gRPC's
status does, which its handler sets before the stream ends.

```tondo
import std.http
import std.io

var app = http.App()
// Headers after the body, as a checksum known only once it is written.
app.get("/report", (_) {
    let trailers = http.Trailers()
    trailers.set("X-Checksum", "9a0364b9")
    http.Reply.stream(io.reader("line 1\nline 2\n"), contentType: "text/plain", trailers: some(trailers))
})
app.listen(port: 8080)?
```

#### Trailers

```tondo
pub fn Trailers(): Trailers
```

Trailers to set.

#### Trailers.set

```tondo
pub fn Trailers.set(self, name: String, value: String)
```

Sets a trailer, replacing one of that name.

#### Trailers.all

```tondo
pub fn Trailers.all(self): Map[String, String]
```

The trailers set, by name.

### SameSite

```tondo
pub enum SameSite {
    Lax
    Strict
    None
}
```

When a browser sends a cookie from another site's page: `Lax` with the
links that lead to this one, `Strict` never, `None` always, which takes
`Secure`.

```tondo
import std.http
import std.time

var app = http.App()
app.post("/login", (_) {
    http.Reply.redirect("/").cookie(
        "session",
        "token",
        maxAge: some(time.hours(8)),
        secure: true,
        sameSite: http.SameSite.Strict,
    )
})
app.listen(port: 8080)?
```

### App

```tondo
pub type App = {
    // private fields
}
```

The routes, middleware and documentation of a server. Routes match a
method and a path whose `:name` segments are parameters and whose last
may be `*name`, the rest of the path; a route with fixed segments wins
over one with parameters.

```tondo
import std.console
import std.http

pub type Todo = { id: Int, title: String }

var api = http.App()
api.get("/todos/:id", (request) { http.Reply.json(Todo { id: request.intParam("id")?, title: "Write docs" }) })
api.post("/todos", (request) { http.Reply.json(request.json[Todo]()?, status: 201) })

var app = http.App()
app.cors(["https://example.com"])
app.compress()
app.mount("/api", api)
app.get("/health", (_) { http.Reply.text("ok") })
console.println(app.client().get("/api/todos/1")?.text()?)
```

#### App.websocket

```tondo
pub fn App.websocket(mut self, path: String, handler: fn(Request, WebSocket): !HttpError)
```

Answers WebSockets at a path: the handler talks to each until it
returns, with the request that opened it, and inside the contexts the
middleware set, as a route's handler is.

#### App

```tondo
pub fn App(): App
```

An app without routes.

#### App.get

```tondo
pub fn App.get(mut self, path: String, handler: fn(Request): Reply ! Problem)
```

Routes `GET` and `HEAD` requests of `path`, whose `:name` segments are
parameters and whose last may be `*name`, the rest of the path.

#### App.post

```tondo
pub fn App.post(mut self, path: String, handler: fn(Request): Reply ! Problem)
```

Routes `POST` requests of `path`, whose `:name` segments are parameters.

#### App.put

```tondo
pub fn App.put(mut self, path: String, handler: fn(Request): Reply ! Problem)
```

Routes `PUT` requests of `path`, whose `:name` segments are parameters.

#### App.patch

```tondo
pub fn App.patch(mut self, path: String, handler: fn(Request): Reply ! Problem)
```

Routes `PATCH` requests of `path`, whose `:name` segments are
parameters.

#### App.delete

```tondo
pub fn App.delete(mut self, path: String, handler: fn(Request): Reply ! Problem)
```

Routes `DELETE` requests of `path`, whose `:name` segments are
parameters.

#### App.route

```tondo
pub fn App.route(
    mut self,
    method: String,
    path: String,
    handler: fn(Request): Reply ! Problem,
    operation: String = "",
    schemas: Map[String, String] = [:],
)
```

A route of a method, or of any with `"*"`; `operation` is its OpenAPI
operation object and
`schemas` the JSON Schemas it refers to by name. The compiler writes both
for a route with a literal path.

#### App.use

```tondo
pub fn App.use(mut self, middleware: fn(Request, fn(Request): Reply ! Problem): Reply ! Problem)
```

Middleware, around every route of this app and its answers to paths
with no route: it can answer by itself or call `next`. The first added
is the outermost.

```tondo
import std.http
import std.log
import std.time

var app = http.App()
// Each request through it: timed, and its status logged.
app.use((request, next) {
    let started = time.now()
    let reply = next(request)?
    log.info("served", ["path": request.path(), "status": "{reply.status()}", "took": "{started.elapsed()}"])
    reply
})
app.get("/", (_) { http.Reply.text("hello") })
```

#### App.cors

```tondo
pub fn App.cors(
    mut self,
    origins: Array[String],
    headers: Array[String] = [],
    exposed: Array[String] = [],
    credentials: Bool = false,
    maxAge: time.Duration = time.seconds(600),
)
```

Lets pages of other origins call the app, as browsers ask it (CORS): it
answers their preflight `OPTIONS` and lets them read its replies.
`origins` are the origins it lets in, as `https://example.com`, or `*`
for any; `headers` those the pages may send, all they ask for when
empty; `exposed` those of a reply they may read; `credentials` whether
they send cookies; and `maxAge` how long a browser keeps a preflight's
answer.

#### App.mount

```tondo
pub fn App.mount(mut self, prefix: String, other: App)
```

The routes of another app under a prefix, with its middleware around
them.

#### App.grpc

```tondo
pub fn App.grpc[S: GrpcService](mut self, service: S)
```

Serves a gRPC service, which `std.grpc` makes from a service of a
`.proto` file: `app.grpc(users.userService(Users { db }))`.

#### App.files

```tondo
pub fn App.files(mut self, prefix: String, files: fs.FileSystem, listing: Bool = false)
```

Serves the files of a tree at `{prefix}/...`: `GET` and `HEAD`, with
ranges, so downloads resume, the content type of each file's extension
and an `ETag`. A folder's path answers with its `index.html`, or, with
`listing`, with a page of its entries, to open them or download the
files. Paths cannot leave the tree. `fs.disk("public")` serves a
directory, and `embed.files("public")` the files kept in the executable.

#### App.uploads

```tondo
pub fn App.uploads(mut self, prefix: String, directory: String)
```

Takes uploads into a directory at `{prefix}/...`: `PUT` writes its body
to that file, from the byte a `Content-Range: bytes start-end/total`
says, so an upload resumes after a `HEAD` of the files' route tells how
much arrived; `POST` with a multipart form saves each file part under
that folder by its file name. Both answer with the JSON of what they
wrote, `201` for a new file.

#### App.compress

```tondo
pub fn App.compress(mut self, minimum: Int = 1024)
```

Compresses the replies a client takes compressed, with zstd or gzip as
its `Accept-Encoding` prefers: text, JSON, JavaScript, XML and SVG of
`minimum` bytes or more, and streams of those. A reply already encoded,
a file, events and WebSockets go as they are. Add it before the
middleware whose replies it compresses.

#### App.client

```tondo
pub fn App.client(self): Client
```

A client whose requests this app answers in this process: the app
serves on a Unix socket of its own, with no port, and the client sends
there. A test makes its requests with it as it would to a server:
`app.client().get("/users/1")?.json[User]()?`.

#### App.trustProxies

```tondo
pub fn App.trustProxies(mut self, networks: ...net.Network)
```

Trusts the proxies of these networks, such as the app's load balancer,
for the client's address they write: see `Request.clientIp`.

#### App.openApi

```tondo
pub fn App.openApi(
    mut self,
    path: String = "/openapi.json",
    title: String = "API",
    version: String = "1.0",
)
```

Serves the OpenAPI document of the routes the compiler described, at
`path`.

#### App.listen

```tondo
pub fn App.listen(
    self,
    port: Int = 8080,
    host: String = "0.0.0.0",
    tls: Tls? = none,
    socket: String? = none,
    limits: Limits? = none,
    grace: time.Duration = time.seconds(30),
): !HttpError
```

Serves until the task is cancelled or the server stops; `port` 0 takes
any free port. With `socket`, it serves on that Unix socket instead,
which it makes and removes when it stops. `limits` bounds what each
client sends, `Limits.standard()` without it. A client that goes away
before its reply cancels the task that answers it: the handler stops at
its next wait and runs its `defer`s. `Interrupt` (Ctrl-C) or `Terminate`
first drains the server, as `Server.stop(grace)` does, and then ends
the program as it would have; a second one ends it at once.

#### App.start

```tondo
pub fn App.start(
    self,
    port: Int = 0,
    host: String = "127.0.0.1",
    tls: Tls? = none,
    socket: String? = none,
    limits: Limits? = none,
): Server ! HttpError
```

Starts serving in a task of its own, for a program that goes on, such
as a test; `Server.stop()` ends it.

### GrpcService

```tondo
pub trait GrpcService {
    fn serveOn(self, app: mut App)
}
```

What `App.grpc` serves, which `std.grpc` implements.

```tondo
import std.grpc
import std.http

// A service of a `.proto` file, or one of std.grpc, is served by an app.
let health = grpc.Health()
var app = http.App()
app.grpc(health.service())
app.listen(port: 50051)?
```

### Saved

```tondo
pub type Saved = {
    path: String
    size: Int
}
```

A file an upload wrote: its path under the directory and its size, as
the JSON of the reply gives it to a client.

```tondo
import std.console
import std.fs
import std.http

var app = http.App()
app.uploads("/files", fs.createTemporaryDirectory()?)
let client = app.client()
let saved = client.put("/files/notes/today.txt", body: some(http.Body.text("buy tea")))?.json[Array[http.Saved]]()?
for file in saved {
    console.println("{file.path}: {file.size} bytes")
}
```

Implements `serialization.Encode`, `serialization.Decode`.

### Limits

```tondo
pub type Limits = {
    body: Int
    head: Int
    headTimeout: time.Duration
    bodyTimeout: time.Duration
    replyTimeout: time.Duration
}
```

What a server allows each client, so that a large or slow one cannot
exhaust it: the bytes of a request's body, more of which is `413`; the
bytes of its head, its line and headers, more of which is `431`; the
time to send a head, or a TLS handshake, which also ends a connection
kept open without a request; the longest a body may stop arriving,
after which it is `408`; and the longest a client may stop taking a
reply, after which the connection closes and the handler is cancelled.
`Limits.standard() with { body: 100_000_000 }` changes one;
`Request.limitBody` changes the body's for a route.

```tondo
import std.http
import std.time

var app = http.App()
// Videos up to 1 GB on this route; 10 MiB on the others.
app.post("/videos", (request) { http.Reply.text("{request.limitBody(1_000_000_000).saveTo("video.mp4")?} bytes") })
let limits = http.Limits.standard() with { headTimeout: time.seconds(10) }
app.listen(port: 8080, limits: some(limits))?
```

#### Limits.standard

```tondo
pub fn Limits.standard(): Limits
```

10 MiB of body, 64 KiB of head, 30 seconds to send a head, and 30
seconds that a body may stop arriving or a reply stop being taken.

### Server

```tondo
pub type Server = {
    // private fields
}
```

A server that `App.start` started.

```tondo
import std.console
import std.http
import std.time

var app = http.App()
app.get("/health", (_) { http.Reply.text("ok") })
// A server that runs while the program goes on, as a test's.
let server = app.start()?
console.println("{server.url()}: {http.get("{server.url()}/health")?.text()?}")
server.stop(grace: time.seconds(5))
```

#### Server.address

```tondo
pub fn Server.address(self): String
```

The address it listens on, with its port: `127.0.0.1:43017`.

#### Server.url

```tondo
pub fn Server.url(self): String
```

Its URL for a client on the same machine: `http://127.0.0.1:43017`; for
one on a Unix socket, `http://localhost`, with the socket given to the
client.

#### Server.stop

```tondo
pub fn Server.stop(self, grace: time.Duration = time.seconds(30))
```

Stops taking connections and waits until the server stopped: each
connection open ends the requests it has, those that arrived too, and
closes, with a GOAWAY on HTTP/2; after `grace`, those left close as
they are.

### Tls

```tondo
pub alias Tls = net.Tls
```

What a server shows and asks of TLS: `net.Tls`.

```tondo
import std.http

var app = http.App()
app.get("/", (_) { http.Reply.text("hello over HTTPS") })
app.listen(port: 8443, tls: some(http.Tls.files("cert.pem", "key.pem")))?
```

### TlsVersion

```tondo
pub alias TlsVersion = net.TlsVersion
```

A version of TLS: `net.TlsVersion`.

```tondo
import std.http

var app = http.App()
app.get("/", (_) { http.Reply.text("TLS 1.3 only") })
let tls = http.Tls.files("cert.pem", "key.pem", minimumVersion: http.TlsVersion.Tls13)
app.listen(port: 8443, tls: some(tls))?
```
