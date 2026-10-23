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
std.http       HTTP client and server, WebSockets (`tondo guide http`)
std.regex      regular expressions
std.encoding   Base64, hexadecimal, URL text
std.hash       SHA-1, SHA-256, SHA-512, HMAC
std.random     seeded and entropy generators
std.uuid       UUID v4, v5, v7
std.format     a text builder
std.math       floating-point math and integers' gcd and lcm
std.sync       Mutex, Atomic, shared collections (`tondo guide tasks`)
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
