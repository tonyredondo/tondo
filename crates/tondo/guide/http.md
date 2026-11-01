# HTTP

`std.http` is a client and a server for HTTP/1.1 and HTTP/2 over TLS, with
WebSockets. `tondo api std.http` shows all of it.

## Client

```tondo
import std.console
import std.http
import std.time

pub type Repo = { name: String, stars: Int }
pub type NewIssue = { title: String }

fn main(): !http.HttpError {
    let repo: Repo = http.get("https://api.example.com/repos/1")?.json()?

    // A client keeps connections, a base URL and headers.
    let api = http.Client.new(
        baseUrl: "https://api.example.com",
        headers: ["Authorization": "Bearer token"],
        timeout: time.seconds(30),
    )
    let created = api.post("/issues", body: http.Body.json(NewIssue { title: "Bug" }))?
    console.println("{repo.name} {created.status()}")

    // A status of 400 or more is an error that carries the response.
    match api.get("/missing") {
        err(http.HttpError.Status(response)) => console.println("{response.status()}")
        other => console.println("{other}")
    }

    // Bodies stream: lines, Server-Sent Events, or into a file.
    for event in api.get("/events")?.events() {
        console.println(event?.data)
    }
    let upload = http.Body.multipart([
        http.Part.field("title", "report"),
        http.Part.file("file", path: "report.pdf"),
    ])
    _ = api.post("/upload", body: upload)?
}
```

- A response's body is read once: keep what `text()` gives to read it
  again, as `text.lines()`.
- `client.use((request, next) { ... next(request)? })` wraps each request
  of the client, as middleware wraps routes: change its `headers` with
  `request with { headers: ... }`, time it, see the response or error of
  `next`, or answer without sending. The first added is the outermost.
- `text()` and `lines()` read the `charset` of the content type: UTF-8, or
  Windows-1252 for it and for Latin-1 and ASCII, as browsers do. Another
  charset is an error that says to read the bytes with `bytes()`.

## Server

```tondo
import std.console
import std.fs
import std.http

pub type NewUser = { name: String }
pub type User = { id: Int, name: String }

fn main(): !http.HttpError {
    var app = http.App.new()

    // Typed handlers: route segments, query parameters and the JSON body
    // come from the parameters; the result becomes the reply.
    app.get("/users/:id", (id: Int) { find(id) })                    // User? → 404 without one
    app.get("/search", (q: String, page: Int?) { "{q} {page}" })     // ?q=...&page=...
    app.post("/users", (input: NewUser) { User { id: 1, name: input.name } })

    // A handler can also take the request and build the reply.
    app.get("/hello/:name", (request) {
        http.Reply.text("hello {request.param("name")}")
    })

    // Middleware wraps every route.
    app.use((request, next) {
        let reply = next(request)?
        console.println("{request.method()} {request.route()} {reply.status()}")
        reply
    })

    app.files("/downloads", fs.disk("public"))    // GET with ranges: downloads resume
    app.files("/browse", fs.disk("public"), listing: true)   // and a page of each folder
    app.uploads("/downloads", "public")       // PUT (resumable) and multipart POST
    app.websocket("/chat", (_, socket) {
        for message in socket {
            socket.sendText("echo: {message?}")?
        }
    })
    app.openApi(title: "Users", version: "1.0")   // GET /openapi.json
    app.listen(port: 8080)?
}

fn find(id: Int): User? {
    if id == 1 { some(User { id, name: "Ada" }) } else { none }
}
```

- Tests send requests with `app.client()`, a `Client` the app answers in
  the same process, with no port: `app.client().get("/users/1")?`.
- `request.json[T]()` answers `422` when the body breaks the rules of `T`
  (`@validate.Min(18)`, `tondo api std.validate`), with each in `errors`.
- `request.queryAs[Search]()` reads the query into a type, and
  `request.formAs[T]()` a form: `page_size` fills `pageSize`, and a name
  that repeats fills a list. A field that may be missing is optional, as
  `tags: Array[String]?`; a missing or unreadable one is a 400 that names
  the parameter. A multipart form reads the same way, each file into a
  field of type `http.UploadedFile`, in memory; `request.uploads()` reads
  its parts one by one, for files too large for memory.
- A body is read once, as it arrives: a middleware that reads it gives it
  on with `next(request.withBody(bytes))`, and the handler reads it again.
- A middleware reads and changes the reply: `reply.status()`,
  `reply.headers()` and `reply.body()` (in memory: text, JSON, bytes;
  `none` for a stream or a file), `reply.withStatus(code)` and
  `reply.withBody(bytes)`. A handler's failure is `err(problem)`, which
  `http.Reply.problem(problem)` turns into the reply the server would send,
  to add headers to it. `header` replaces a header; `addHeader` adds a value,
  as `Set-Cookie` and `Vary` take.
- A parameter takes a whole segment, `/files/:file`; a route added twice
  is a panic, and `/foo/` is `/foo`.
- Each client is limited: by default a body over 10 MiB is 413, a head over
  64 KiB is 431, a head has 30 seconds to arrive, a body that stops arriving
  for 30 seconds is 408, and a client that takes none of its reply for 30
  seconds loses the connection.
  `app.listen(limits: http.Limits.standard() with { body: 100_000_000 })`
  changes them, and `request.limitBody(bytes)` the body's for one route,
  before it is read.
- A client that goes away cancels its handler: it stops at its next wait
  and runs its `defer`s.
- `app.compress()` compresses text and JSON replies with zstd or gzip, as
  the client's `Accept-Encoding` prefers; it compresses what the middleware
  added after it returns.
- `app.cors(["https://example.com"])` lets pages of that origin, or of any
  with `"*"`, call the app from a browser: it answers their preflight and
  lets them read each reply. `headers`, `exposed`, `credentials` (cookies)
  and `maxAge` say the rest. Add it first, so that what other middleware
  answers, as a 401, carries it too.
- `app.route("GET", "/api/*rest", http.proxy("http://backend:8080", strip:
  "/api"))` passes requests on to another server and its responses back.
- Routes use `:name` and `*rest` segments (`{name}` would interpolate).
- A handler fails with `http.Problem` (`Problem.notFound()`,
  `Problem.badRequest(message)`), sent as `application/problem+json`; other
  errors become 500s. Bad input to a typed handler is a 400 that says why.
- A handler or middleware that panics answers 500; the panic is reported on
  standard error with the request's method and path, and the server goes on.
- `app.mount("/admin", adminApp)` adds another app's routes with its own
  middleware. `app.start()` serves in a task of its own and gives a `Server`
  with `url()` and `stop()`, for tests. `stop()` drains: the requests that
  arrived are answered and the connections close, for up to `grace` (30
  seconds), so a reply that never ends, as a stream a client still holds,
  keeps it waiting that long.
- `app.files("/", Site)` serves the files of `const Site: fs.FileSystem =
  embed.files("public")`, kept in the executable, as it serves a directory
  of the disk; a folder answers with its `index.html` (`tondo guide stdlib`).
- `request.cookie("session")` reads a cookie and
  `reply.cookie("session", token, maxAge: time.hours(1))` sets one, `HttpOnly`
  and `SameSite=Lax` unless told otherwise; `http.Client.new(cookies: true)`
  keeps the cookies it gets, as a browser does.
- `request.clientIp()` is the client's address. Behind a load balancer,
  `app.trustProxies(net.network("10.0.0.0/8")?)` names its network, so the
  address it writes in `X-Forwarded-For` is believed; without it, no header
  is, since any client can write one.
- `request.url()` is the URL the request was for, from `scheme()`,
  `host()`, the path and `rawQuery()`; behind a trusted proxy,
  `X-Forwarded-Proto` and `X-Forwarded-Host` give them.
- A reply written while it is sent: `let (out, body) = io.pipe()`, a task
  that writes to `out` and closes it, and `http.Reply.stream(body)`.
- `request.route()` is the template of the route that answers, as
  `/hello/:name`, which middleware sees before `next`: name metrics and
  traces by it, not by the path, which grows with every parameter.
- `app.listen(socket: path)` serves on a Unix socket, and
  `http.Client.new(baseUrl: "http://agent", socket: path)` sends through one.
- Ctrl-C or `docker stop` cancel the program: `listen` stops accepting, the
  requests in flight run their `defer`s, and it ends with 130 or 143.
- TLS: `app.listen(port: 443, tls: http.Tls.files("cert.pem", "key.pem"))`,
  or `http.Tls.pem(certificate, key)` with the PEM text of an environment
  variable. `clients: "ca.pem"` requires a certificate of each client, which
  `request.clientCertificate()` gives; a client shows its own with
  `http.Client.new(identity: http.Tls.files("me.pem", "me.key"))`.
- Certificates of Let's Encrypt, obtained and renewed by the server itself:
  `app.listen(port: 443, tls: acme.tls(["example.com"], email: "me@example.com"))`
  with `import std.acme`; only port 443 has to be reachable. They are kept in
  the user's cache directory, and `directory: acme.LetsEncryptStaging` tries
  it without the real limits. `net.Certificates` change while a server shows
  them, for certificates renewed some other way.

## GraphQL

`std.graphql` makes a GraphQL server of your types: the fields and `pub`
methods of the query's type, and of each type they give, are the schema's
fields; a method's parameters are its arguments, and `///` its description.

```tondo
import std.graphql
import std.http
import std.sql

pub type Book = { id: Int, title: String }

/// What the API answers.
pub type Library = { db: sql.Database }

/// The books whose title has the text.
pub fn Library.books(self, matching: String?): Array[Book] ! sql.SqlError {
    let pattern = "%{matching.unwrapOr("")}%"
    self.db.query[Book]("SELECT id, title FROM books WHERE title LIKE {pattern}")?
}

fn main(): !http.HttpError {
    let db = match sql.open("sqlite:books.db") {
        ok(opened) => opened
        err(error) => panic("{error}")
    }
    let schema = graphql.schema(Library { db })
    var app = http.App.new()
    app.post("/graphql", schema.handler())
    app.get("/graphql", schema.handler())       // GraphiQL in a browser
    app.listen(port: 8080)?
}
```

- `.mutation(root)` adds the methods of another value as mutations;
  `schema.sdl()` writes the schema; `schema.execute(document, variables)`
  runs a query in the program, as a test does. Write documents as raw
  strings, `r"{ books { title } }"`: their braces are not interpolation.
- An optional parameter is an optional argument; a default value is not
  seen. A method's error leaves its field null with the error's message
  and path. A `sql.Database` or a `sync.Mutex` in a root is carried, not
  shown.

## gRPC

A `service` of a `.proto` file among the sources gives, in its module, a
trait of its calls, a function that makes a server's service of a value
that implements it, and a client. Messages are records of `std.protobuf`;
a failure is a `grpc.Status`.

```text
// src/api/users.proto
syntax = "proto3";
package users.v1;
message GetUser { int64 id = 1; }
message User { int64 id = 1; string name = 2; }
service UserService {
  rpc Get(GetUser) returns (User);
  rpc Watch(GetUser) returns (stream User);
}

// src/main.to
import std.console
import std.grpc
import std.http
import std.time
import app.api.users

type Users = { names: Map[Int, String] }

impl users.UserService for Users {
    fn get(self, request: users.GetUser): users.User ! grpc.Status {
        match self.names.get(request.id) {
            some(name) => users.User { id: request.id, name }
            none => fail grpc.Status { code: grpc.Code.NotFound, message: "no user {request.id}" }
        }
    }

    fn watch(self, request: users.GetUser, out: Sender[users.User]): !grpc.Status {
        out.send(self.get(request)?)
    }
}

fn main(): !(http.HttpError | grpc.Status | time.TimedOut) {
    var app = http.App.new()
    app.grpc(users.userService(Users { names: [7: "Ada"] }))   // beside other routes
    let server = app.start()?
    let client = users.UserServiceClient.connect("http://{server.address()}")?
    let user = within time.seconds(2) { client.get(users.GetUser { id: 7 }) }?
    console.println(user.name)
    for each in client.watch(users.GetUser { id: 7 }) {
        console.println(each?.name)
    }
    server.stop()
}
```

- A call's method takes the request and gives `Out ! grpc.Status`; with a
  stream from the server it takes `out: Sender[Out]` and gives
  `!grpc.Status`; with one from the client it takes
  `input: grpc.Stream[In]`, read with `for each in input { each? }`.
- The client's call gives `Out ! grpc.Status`, a `grpc.Stream[Out]`, a
  `grpc.Upload[In, Out]` (`send`, then `finish()` for the reply) or a
  `grpc.Duplex[In, Out]` (`send`, `closeSend()`, `replies()`); each takes
  `metadata: [...]` too. `https://` connects with TLS, trusting `trust:`.
- In a method, `grpc.call()` gives the metadata, the peer and the time left;
  `grpc.setHeader` and `grpc.setTrailer` answer with metadata. A panic
  answers `Internal`, and so does another error, with
  `result.mapErr(grpc.Status.from)?`; `details: [protobuf.pack(message)]`
  travel with a status.
- `grpc.connect("http://a:50051,b:50051", balancing:
  grpc.Balancing.RoundRobin)` spreads calls over several servers, or over
  the addresses a name resolves to; `PickFirst`, the default, keeps them on
  the first that answers; round robin resolves the name again every 30
  seconds, so it reaches the servers it gains. An address that fails waits
  as gRPC's backoff
  says, and a call while all wait fails at once with `Unavailable`. A call
  no server began, as one a server going away refused, goes again by
  itself.
- `grpc.connect(target, compression: grpc.Compression.Gzip)` compresses
  what a client sends, and `service.compress()` what a service answers to
  a client that reads gzip; both always read it.
- The same routes answer gRPC-Web, which browsers send, over HTTP/1.1 too,
  as bytes or as Base64 text, and Connect, in bytes or proto3's JSON: a
  unary call is a plain `POST` (`curl -H 'content-type: application/json'
  -d '{"id": 7}' .../users.v1.UserService/Get`), and a method with
  `option idempotency_level = NO_SIDE_EFFECTS;` takes a `GET` too. A page
  of another origin needs `app.cors`.
- A call in a `within` sends its deadline; the method runs within it, and
  the client gets its `within`'s `TimedOut` when it passes. A method whose
  client goes is cancelled: its waits end and its `defer` runs.
- `service.onCall((call, next) { ... next()? })` wraps each call of a
  server: refuse it with `fail`, give the method contexts with `with`, see
  its status. `grpc.connect(target)?.onCall((call, next) { next(call with {
  metadata: more })? })` wraps a client's, and `users.UserServiceClient.on(
  channel)` uses it; `onMessage((call, direction, message) { ... })` sees
  each message as a `reflect.Value`. The first added is the outermost.
- `let health = grpc.Health.new()`, `app.grpc(health.service())` and
  `health.set("users.v1.UserService", grpc.ServingStatus.NotServing)` answer
  `grpc.health.v1`; `app.grpc(grpc.reflection([service, health.service()]))`
  lets grpcurl list, describe and call them without the `.proto` files.
- The value that implements the service is copied into each method, so it
  is `Copy`: hold handles, as a `sql.Database`, not a `Sender`.
