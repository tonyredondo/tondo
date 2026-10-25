# The standard library

`import std.x` brings a module; `tondo api std.x` shows all it has. The
prelude needs no import: methods of `String`, `Char`, `Int`, `Float`,
`Option`, `Result`, `Array`, `Map`, `Set`, ranges and iterators (`tondo api
prelude`).

```text
std.console    print, println, eprintln, readLine, the streams
std.io         Writer, Reader, Buffer: streams for files, processes, memory
std.fs         read and write files, list, open, metadata
std.path       join, parent, fileName, extension
std.env        args, environment, current directory
std.process    run programs and pipelines, capture output
std.time       Duration, sleep, now, UTC dates, ISO 8601
std.json       encode and decode any type, navigate dynamic JSON
std.toml       the same for TOML documents, with dates
std.yaml       the same for YAML (a safe subset), one or many documents
std.messagepack  the same for MessagePack bytes
std.cbor       the same for CBOR (RFC 8949), deterministic if asked
std.serialization  the protocol every format shares, for custom encodings
std.csv        CSV rows and records, streaming and whole
std.log        a logger with levels and fields, text or JSON lines
std.diagnostics  the program's own counters, which `tondo counters` shows
std.http       HTTP client and server, WebSockets (`tondo guide http`)
std.net        TCP and UDP sockets, TLS, names
std.regex      regular expressions
std.encoding   Base64, hexadecimal, URL text
std.hash       SHA-1, SHA-256, SHA-512, HMAC
std.random     seeded and entropy generators
std.uuid       UUID v4, v5, v7
std.format     a text builder
std.math       floating-point math and integers' gcd and lcm
std.sync       Mutex, Atomic, shared collections (`tondo guide tasks`)
std.executor   pools that bound work, actors with a mailbox (`tondo guide tasks`)
std.reflect    values by name at run time (`tondo guide meta`)
std.ffi        C types and memory (`tondo guide ffi`)
std.testing    what a test tells the runner (`tondo guide testing`)
```

## Files, JSON and processes

```tondo
import std.console
import std.fs
import std.json
import std.process

pub type Config = { name: String, port: Int, tags: Array[String] }

fn main(): !(fs.FsError | json.JsonError | process.ProcessError) {
    let config = Config { name: "api", port: 8080, tags: ["web"] }
    fs.writeText("config.json", json.encode(config, pretty: true))?
    let loaded: Config = json.decode(fs.readText("config.json")?)?
    console.println("{loaded.name}:{loaded.port}")

    for line in fs.readText("config.json")?.lines() {
        console.println(line)
    }
    let output = process.command("ls", "-l").output()?
    console.println(output.text()?)
}
```

Any record or enum encodes as JSON when its fields do; `json.Value` handles
JSON of unknown shape. The same types read and write every format:

```tondo
import std.console
import std.time
import std.toml

type Server = { host: String, port: Int, tls: Bool? }
type Config = { name: String, started: time.DateTime, servers: Array[Server] }

fn main(): !toml.TomlError {
    let text = """
        name = "app"
        started = 2026-10-23T08:00:00Z

        [[servers]]
        host = "localhost"
        port = 8080
        """
    let config = toml.decode[Config](text)?
    console.println("{config.servers[0].port} {toml.parse(text)?.find("name")}")
    console.print(toml.encode(config with { name: "copy" })?)
}
```

TOML has no null: a `none` field is left out when written.

## The network

```tondo
import std.console
import std.net
import std.time

fn handle(connection: net.Stream): !net.NetError {
    for {
        match connection.readLine(timeout: time.seconds(30))? {
            some(line) => connection.write("echo {line}\n")?
            none => break
        }
    }
    connection.close()?
}

fn serve(listener: net.Listener) {
    for connection in listener {
        spawn detached handle(connection)
    }
}

fn main(): !net.NetError {
    let listener = net.listen("127.0.0.1", 0)?
    spawn detached serve(listener)
    let stream = net.connect("localhost", listener.address().port, timeout: time.seconds(5))?
    stream.write("hello\n")?
    console.println("{stream.readLine()?}")
    stream.close()?
    listener.close()

    let socket = net.bind("127.0.0.1", 0)?
    socket.sendTo("ping".bytes(), socket.address())?
    let (packet, from) = socket.receive(timeout: time.seconds(1))?
    console.println("{packet.length()} bytes from {from}")
}
```

- `spawn detached` serves each connection in a task nobody waits for; a
  plain `spawn` in the loop would wait for each before the next (`W1013`).
- `connect` resolves the name and tries each address; `connectTls(host,
  port, trust: "ca.pem")` verifies the certificate, and `listen(host, port,
  tls: net.Tls.files("cert.pem", "key.pem"))` serves TLS.
- Every call that waits takes its own `timeout:`; there is none otherwise.
  `reader()` and `writer()` give the connection as `std.io` streams.

## Time, logging and CSV

```tondo
import std.csv
import std.io
import std.log
import std.time

fn main(): !(csv.CsvError | time.TimeError) {
    let started = time.now()
    let logger = log.Logger.new(level: log.Level.Debug)
    let date = time.parseDateTime("2026-10-22T08:00:00+02:00")?
    logger.info("starting", ["date": "{date}"])
    for record in csv.records(io.reader("name,age\nada,36\n")) {
        let row = record?
        logger.debug("row", ["name": row.getOr("name", "")])
    }
    logger.info("done", ["took": "{started.elapsed()}"])
}
```

## Watching a running program

A program started with `TONDO_DIAGNOSTICS=1` can be watched; without it,
it pays nothing for it. `tondo ps` lists those running; `tondo counters
app` (a name or a process ID) shows theirs every second: memory, the cycle collector,
tasks, workers, calls of C, sockets, files, HTTP requests, CPU. `--json`
writes a line per second for a script or a model. A program adds its own:

```tondo
import std.diagnostics

fn handle(order: String) {
    diagnostics.counter("orders").add()
    diagnostics.gauge("last order size").set(order.length())
}

fn main() {
    handle("two books")
}
```

A name gives the same counter from any task, so nothing is passed around.
A `Counter` only grows (its rate shows per second); a `Gauge` goes up and
down.

`tondo trace app --duration 10`, or `tondo trace -- app.to` from its start,
records what the tasks do: `app-<pid>.json`, a timeline for
https://ui.perfetto.dev with each run of a task and where it waited, and
`app-<pid>.folded`, a CPU profile, one stack a line with its samples, which
shows where the time goes.

`tondo dump app` writes `app-<pid>.dump.json`: every task, where it waits or
runs, where it was spawned and the locks it holds, the counters, and the
live memory by where it was allocated. `tondo dump read <file>` shows it
(`tasks`, `task <id>`, `heap`, `--json`). With `TONDO_DUMP=<dir>`, a
program that panics, crashes or deadlocks leaves its dump there. The dump
of a `--debug` build has every object: `objects` by type, and `object
<address>` with what keeps it alive, `task 1 -> Map[...] -> String`.

## Debugging

`tondo debug app.to` builds it with `--debug` (unoptimized, with a probe at
each line; a build without it has none) and stops before `main`. Commands,
one a line, typed or piped, `--json` for a line of JSON each:

```text
break app.to:12 if count > 3    stop where line 12 starts, when count > 3
continue | step | next | finish run, into calls, over them, out
locals | print user | stack     the bindings that hold a value, the calls
tasks | pause | delete 1 | quit
```

When one task stops, all stop at their next line, and go on together.
`tondo dap` is the same debugger for an editor (Debug Adapter Protocol).

## Live builds

`tondo run app.to --live` runs a live build, whose code changes while it
runs: after an edit, `tondo reload app` puts the new code in, keeping the
program's state, and says which functions changed. `tondo run app.to
--watch` does it whenever a file is saved. Calls that start afterwards run
the new code, closures made before included, and a call already running
moves to it at its next statement, with its values: a loop in `main` goes
on with the new body. Where the new code needs a binding the old one does
not have there, the call stays on the old code until it reaches another
statement, and reload says which lines and why. A type may gain or lose
fields and variants: the values running calls hold convert as the calls
move, their parameters included, field by field, a new field starting
empty (0, `""`, `[]`, `none`) and a number of another numeric type
converting as `T(x)` does, and reload says what it did to them. Closures
follow too, when another closure comes before them or what they capture
changes shape. A value kept in a `Ref`, a channel, a task or `std.sync`
does not convert: reload says where and to start the program again, which
`--watch` does. Under `tondo debug --live app.to`, `reload` works where
the program stopped: edit the function, reload, continue; or `restart`,
which runs the stopped function again from its first line, with its new
code.
