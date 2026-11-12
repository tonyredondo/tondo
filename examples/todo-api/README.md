# todo-api: an HTTP API over a database

A JSON API for a list of tasks, kept in SQLite. It is about 150 lines of
Tondo in three modules, and everything it uses came with `tondo`: the HTTP
server, the JSON, the database, the validation, the logging and the tests.

```text
todo-api/
  tondo.toml
  src/
    main.to                       opens the database and listens
    api/api.to                    the routes, and the middleware around them
    store/store.to                the tasks, and the queries that keep them
    store/store_test.to           unit tests, against a database in memory
    store/migrations/001_tasks.sql    the schema, embedded in the executable
  tests/api.to                    the API tested from outside, over HTTP
```

## Run it

```sh
tondo run examples/todo-api                       # port 8080, tasks.db
tondo run examples/todo-api -- 9000 sqlite::memory:
tondo test examples/todo-api
```

```text
$ curl -X POST localhost:8080/tasks -d '{"title": "write the manual"}'
{"id":1,"title":"write the manual","done":false}

$ curl -X POST localhost:8080/tasks -d '{"title": ""}'
{"status":422,"detail":"the value breaks the rules of its type: title: must have at least 1 character","errors":[{"path":"title","message":"must have at least 1 character"}]}

$ curl -X PATCH localhost:8080/tasks/1 -d '{"done": true}'
{"id":1,"title":"write the manual","done":true}

$ curl 'localhost:8080/tasks?done=true'
[{"id":1,"title":"write the manual","done":true}]

$ curl -i localhost:8080/tasks/7
HTTP/1.1 404 Not Found
content-type: application/problem+json
x-request-id: 01a1ccda-54f3-7a06-8d17-5ec28062ff8b

{"status":404,"detail":"not found"}
```

and the server's log, a line for each request:

```text
2026-11-11T23:03:43.071777103Z INFO request request_id=01a1ccda-54df-7902-8e9c-551295516b31 method=POST route=/tasks status=201 ms=0
2026-11-11T23:03:43.077395605Z INFO request request_id=01a1ccda-54e5-7c75-bc13-a0ca8789b251 method=POST route=/tasks status=422 ms=0
2026-11-11T23:03:43.082070181Z INFO request request_id=01a1ccda-54e9-7536-b0ed-550d4968d913 method=PATCH route=/tasks/:id status=200 ms=0
```

`GET /openapi.json` describes the API, written from the routes.

## What to look at

**Types are the API.** `Task`, `NewTask` and `TaskChange` in
[`store.to`](src/store/store.to) are read from JSON and written to it with
nothing derived or registered. `@validate.Length(min: 1, max: 200)` on a
field is checked by the decoder, so a handler never sees a task with an
empty title: the request is answered with 422 and what is wrong before the
handler runs.

**Typed handlers.** In [`api.to`](src/api/api.to),

```text
app.get("/tasks", (done: Bool?) { store.list(db, done) })
app.get("/tasks/:id", (id: Int) { store.find(db, id) })
```

are two whole routes. A parameter named in the path arrives from it, `id`
as an `Int`, and one that is not is read from the query, `done` as a
`Bool?`. A value that does not fit is a 400 that says which one and why.
What the handler gives is written as JSON, and `none` is a 404. The
handlers that need more take the request and build the reply.

**Queries cannot be injected.** A query is a string literal where a
`sql.Query` is expected, and each `{value}` in it is a parameter, never
text:

```text
db.query[Task]("SELECT id, title, done FROM tasks WHERE done = {state} ORDER BY id")
```

The rows are read into `Task`s by the names of their columns.

**A transaction is a closure.** `store.change` reads a task and writes it
back inside `db.transaction(...)`, which commits when the closure ends and
rolls back when it fails or panics.

**A context instead of a parameter.** The middleware sets
`Request.id` for the request it wraps. Every log line written while that
request is answered carries it, in any function, and no function takes it as
a parameter. Two requests answered at once each see their own.

**Errors that say what, and no more.** A handler fails with an
`http.Problem`, which is the reply. A failure of the database is logged in
full and answered as a 500 that gives nothing away.

**The schema travels with the program.** `embed.files("migrations")` keeps
the `.sql` files inside the executable, and `db.migrate` applies those not
yet applied, each once. There is nothing to deploy beside the binary.

**Tests at two levels.** [`store_test.to`](src/store/store_test.to) tests
the queries against `sqlite::memory:`, a database that lives in the test.
[`tests/api.to`](tests/api.to) starts the real server on a free port with
`routes(db).start()`, and talks to it with the HTTP client, seeing only what
a user of the package sees.

## Going further

- `tondo run examples/todo-api --watch` reloads the code as it is saved and
  keeps the server and its database running.
- `TONDO_DIAGNOSTICS=1 tondo run examples/todo-api`, then `tondo ps` names
  the running program, `tondo requests <name>` follows each request through
  its tasks, and `tondo counters <name>` shows the server's counters.
- `tondo build examples/todo-api --static --output todo` makes one file that
  runs in an empty container.
- With `postgres://user:secret@host/tasks` instead of `sqlite:tasks.db`, the
  queries of `store.to` stay as they are; only the migration changes, to
  number rows the way PostgreSQL does.
