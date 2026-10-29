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
- `text()` and `lines()` read the `charset` of the content type: UTF-8, or
  Windows-1252 for it and for Latin-1 and ASCII, as browsers do. Another
  charset is an error that says to read the bytes with `bytes()`.

## Server

```tondo
import std.console
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

    app.files("/downloads", "public")         // GET with ranges: downloads resume
    app.files("/browse", "public", listing: true)   // and a page of each folder
    app.uploads("/downloads", "public")       // PUT (resumable) and multipart POST
    app.websocket("/chat", (socket) {
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

- Routes use `:name` and `*rest` segments (`{name}` would interpolate).
- A handler fails with `http.Problem` (`Problem.notFound()`,
  `Problem.badRequest(message)`), sent as `application/problem+json`; other
  errors become 500s. Bad input to a typed handler is a 400 that says why.
- A handler or middleware that panics answers 500; the panic is reported on
  standard error with the request's method and path, and the server goes on.
- `app.mount("/admin", adminApp)` adds another app's routes with its own
  middleware. `app.start()` serves in a task of its own and gives a `Server`
  with `url()` and `stop()`, for tests.
- `app.embedded("/", Site)` serves the files of `const Site: embed.Files =
  embed.files("public")`, kept in the executable, as `files` serves a
  directory; a folder answers with its `index.html` (`tondo guide stdlib`).
- `request.route()` is the template of the route that answers, as
  `/hello/:name`, which middleware sees before `next`: name metrics and
  traces by it, not by the path, which grows with every parameter.
- `app.listen(socket: path)` serves on a Unix socket, and
  `http.Client.new(baseUrl: "http://agent", socket: path)` sends through one.
- Ctrl-C or `docker stop` cancel the program: `listen` stops accepting, the
  requests in flight run their `defer`s, and it ends with 130 or 143.
- TLS: `app.listen(port: 443, tls: http.Tls.files("cert.pem", "key.pem"))`.
