# The standard library

`import std.x` brings a module; `tondo api std.x` shows all it has. The
prelude needs no import: methods of `String`, `Char`, `Int`, `Float`,
`Option`, `Result`, `Array`, `Map`, `Set`, ranges and iterators (`tondo api
prelude`).

```text
std.console    print, println, eprintln, readLine, the streams
std.cli        the command line read into types, with help; styles, tables, progress, questions
std.io         Writer, Reader, Buffer, pipe, copy, multiWriter: streams of all kinds
std.fs         read and write files, walk, glob, watch, metadata, links, temporaries, trees
std.archive    zip, tar, tar.gz and tar.zst: read, write, extract
std.image      PNG, JPEG, GIF, WebP: resize, fit, fill, crop, rotate, write
std.embed      files of the package kept in the executable, read when it compiles
std.path       join, parent, fileName, extension, clean, relative, glob patterns
std.env        args, environment, directories, processors, hostname, what was built
std.process    run programs and pipelines, capture output, signals
std.time       Duration, sleep, now, dates, time zones, strftime formats
std.json       encode and decode any type, navigate dynamic JSON
std.toml       the same for TOML documents, with dates
std.yaml       the same for YAML (a safe subset), one or many documents
std.messagepack  the same for MessagePack bytes, and a writer in parts
std.cbor       the same for CBOR (RFC 8949), deterministic if asked
std.xml        the same for XML, with attributes and text by annotation; trees
std.serialization  the protocol every format shares, for custom encodings
std.csv        CSV rows and records, streaming and whole
std.log        a logger with levels and fields, text or JSON lines
std.diagnostics  the program's own counters, which `tondo counters` shows
std.http       HTTP client and server, WebSockets (`tondo guide http`)
std.mcp        Model Context Protocol servers from a value's methods
std.graphql    GraphQL servers from your types, with introspection (`tondo guide http`)
std.template   text and HTML templates, compiled or loaded (`tondo guide templates`)
std.sql        SQLite, PostgreSQL, MySQL: queries, rows, migrations (`tondo guide databases`)
std.net        TCP, UDP and Unix sockets, TLS, names, IP addresses and networks
std.acme       certificates of Let's Encrypt, obtained and renewed by a server itself
std.url        URLs and relative links: parts, resolving, the query
std.regex      regular expressions, with `(?i)` flags and replacing by a function
std.text       Unicode normalization: NFC, NFD, NFKC, NFKD
std.encoding   Base64, hexadecimal, URL text, HTML escaping
std.gzip       gzip, whole or as streams; std.zstd the same for Zstandard
std.hash       MD5, SHA-1, SHA-2, HMAC, in parts; FNV, xxHash, MurmurHash3
std.crypto     passwords, sealing with a key, signatures, PEM keys and certificates
std.random     seeded and entropy generators
std.uuid       UUID v4, v5, v7, as text in every format
std.format     a text builder, formats chosen at run time, numbers and plurals per language
std.math       floating-point math and integers' gcd and lcm
std.sync       Mutex, Atomic, shared collections, rate limits, retries (`tondo guide tasks`)
std.validate   rules on the fields of a type, which reading checks: lengths, bounds, patterns
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

Ctrl-C, `kill` or `docker stop` cancel the program, with nothing to write:
every wait ends, the tasks run their `defer`s, and it ends with 130 or 143;
a second one ends it at once. `process.signals(process.Signal.Hangup)` gives
a channel of the signals named instead, which no longer end the program, and
`child.signal(process.Signal.Terminate)` sends one to a process it started;
`process.signal(pid, signal)` to any other. A command started with
`.group()` has a process group of its own, so `cancel()` also ends what it
started, as the tests `go test` runs; the terminal's Ctrl-C then reaches
only the program. `process.which("git")` finds a program as a shell does.

`env.user()` and `env.system()` say who and where the program runs, and
`env.processors(physical: true)` counts the machine's cores.
`env.configDirectory()` and `env.cacheDirectory()` are where a program keeps
the user's settings and what it can make again, and `env.build()` what it
was built from, for a `--version`: the `version` of `tondo.toml`, the git
commit, the target and the packages it uses.

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

A field's annotations hold in every format. `@name("user_id")` gives it
another key, `@ignore` leaves it out, and it must be optional, and
`@omitNone` leaves it out when it is `none`, where JSON and YAML would write
`null`; on a type, `@omitNone` does it for all its optional fields. Reading
gives `none` for a field that is missing, with or without them.
`json.encode(value, htmlSafe: true)` escapes `<`, `>` and `&`, for JSON put
inside a `<script>`.

A YAML key `<<` merges the maps of its anchors into its own. To change a
config file a person wrote, `yaml.load` keeps its text: `set` changes or
adds a value and leaves the comments, the order and the styles as they were.

```tondo
import std.console
import std.yaml

fn main(): !yaml.YamlError {
    var doc = yaml.load("# The service.\nport: 8080   # where it listens\n")?
    console.println("{doc.find("port").andThen((node) { node.comment })}")
    doc.set("port", 9090)?
    doc.set("limits.memory", "512Mi")?
    console.print(doc.text())
}
```

`doc.find(path)` also gives the value's line and column, and
`doc.writeTo(out)` writes the text to a stream without copying it.

XML writes a record as an element and each field as a child, unless
`@xml.Attribute` or `@xml.Text` make it an attribute or the element's text;
`@xml.Name` names an element. An `Array` is elements of one name, with no
wrapper. Reading skips what the type lacks; `xml.parse` gives a tree when
the shape is unknown. No DTD or entity but the predefined ones is read.

```tondo
import std.console
import std.xml

@xml.Name("rss")
pub type Feed = {
    @xml.Attribute
    version: String
    @xml.Name("item")
    items: Array[Item]
}

pub type Item = {
    @xml.Attribute
    id: Int
    @xml.Text
    title: String
}

fn main(): !xml.XmlError {
    let text = xml.encode(Feed { version: "2.0", items: [Item { id: 1, title: "Tea & cake" }] })
    console.println(text)
    let feed = xml.decode[Feed](text)?
    let root = xml.parse(text)?
    for item in root.children("item") {
        console.println("{feed.items[0].title} {item.attribute("id")} {item.text()}")
    }
}
```

## Command lines and the terminal

A record is a command: its fields are options, `--kebab-case`, and with
`@cli.Argument` positional arguments; an enum is subcommands, one a
variant. The `///` documentation is the help, which `-h` shows.

```tondo
import std.cli
import std.console

/// Builds and serves static sites.
pub enum Site {
    /// Builds the site.
    Build(Build)
    /// Serves the site.
    Serve { port: Int? }
}

pub type Build = {
    /// The site's directory.
    @cli.Argument
    source: String?
    /// Where to write it.
    @cli.Option(short: "d", default: "public")
    destination: String
    /// Writes the drafts too.
    drafts: Bool
}

fn main() {
    match cli.parse[Site]() {
        Site.Build(build) => console.println(cli.markup("[green]✓[/] built into [bold]{build.destination}[/]"))
        Site.Serve { port } => console.println("serving on {port.unwrapOr(8080)}")
    }
}
```

- `Bool` fields are flags, `T?` optional, `Array[T]` repeated, enums
  without data a choice, `time.Duration` written `30s` or `1m30s`;
  `cli.parse` ends with 2 and the usage on a wrong line,
  `cli.parseArgs(args)` gives the error instead.
- `@cli.Option(env: ["DD_API_KEY", "DATADOG_API_KEY"])` reads the first of
  those variables that is set when the option is missing; the help shows
  them.
- A command's options also work after its subcommands: `app run --verbose`
  as `app --verbose run`.
- An `Array[String]` field with `@cli.Rest` gets what the command does not
  read, as written, to pass to another program: what follows `--`, options
  it does not declare and extra arguments.
- `cli.markup("[bold red]x[/]")` styles a block and returns to the style
  before; interpolated values are text, and a wrong style is a compile
  error. No codes off a terminal or with `NO_COLOR`.
- `cli.table(records)`, `cli.progress("Building", total: n)`,
  `cli.ask`, `cli.confirm`, `cli.choose`, `cli.secret`,
  `cli.completions[Site](cli.Shell.Bash)`.

## MCP servers

Each `pub` method of a value is a tool, its `///` the description; with one
record parameter, the record's fields are the arguments, their `///` in the
schema. `@mcp.Resource(uri: ...)` and `@mcp.Prompt` make resources and
prompts.

```tondo
import std.mcp

/// Small arithmetic.
pub type Calculator = {}

pub type Sum = {
    /// The numbers to add.
    numbers: Array[Int]
}

/// Adds numbers.
pub fn Calculator.add(self, input: Sum): Int {
    var total = 0
    for number in input.numbers {
        total += number
    }
    total
}

fn main(): !mcp.McpError {
    mcp.serve(Calculator {})?
}
```

- `mcp.serve` speaks over standard input and output; `console` goes to
  standard error meanwhile. `mcp.handler(value)` serves the same on a route
  of `std.http`.
- A `T ! E` that fails is the tool's error, with its `Display`; a panic too,
  and the server goes on.

## Files in the executable

`std.embed` reads files of the package when the program compiles and keeps
them in the executable, which then needs nothing next to it:

```text
import std.embed
import std.fs
import std.http

const Help: String = embed.text("help.txt")          // a UTF-8 file
const Logo: Array[Byte] = embed.bytes("logo.png")    // any file
const Site: fs.FileSystem = embed.files("public")    // a directory, but hidden files

fn main(): !http.HttpError {
    var app = http.App.new()
    app.files("/", Site)          // from memory; "/" is index.html
    app.listen(port: 8080)?
}
```

The path is a literal, relative to the file that writes it, inside the
package. `Site` is a tree of `std.fs` that only reads:
`Site.readText("css/site.css")`, `Site.list()`, `Site.walk()`. A changed file rebuilds `tondo run` and
reloads `--watch`; `tondo graph` shows what each module embeds.

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
  tls: net.Tls.files("cert.pem", "key.pem"))` serves TLS. A protocol that
  starts plain and asks for TLS, as PostgreSQL's, goes on with
  `stream.startTls(host)`.
- Every call that waits takes its own `timeout:`; there is none otherwise.
  `reader()` and `writer()` give the connection as `std.io` streams.

## Rules of a type

```tondo
import std.console
import std.json
import std.validate

pub type Signup = {
    @validate.Length(min: 3, max: 20)
    name: String
    @validate.Email
    email: String
    @validate.Min(18)
    age: Int
}

fn main() {
    match json.decode[Signup](r"""{"name": "al", "email": "x", "age": 12}""") {
        ok(signup) => console.println("welcome {signup.name}")
        err(error) => console.println("{error.violations}")
    }
}
```

- Reading checks the rules, from any format, a form, the command line or the
  arguments of an MCP tool: no value read from outside breaks them, and
  `request.json[T]()` answers `422` with each. `validate.check(value)`
  checks a value the program made.
- Whether a field is required is its type, `T` or `T?`, and a closed set of
  values is an enum; a rule on a field it does not fit is `E1908`.

## Secrets, passwords and signatures

```tondo
import std.console
import std.crypto

fn main(): !crypto.CryptoError {
    let stored = crypto.hashPassword("hunter2")             // Argon2id, to keep
    console.println("{crypto.verifyPassword("hunter2", stored)}")

    let key = crypto.SecretKey.generate()                    // AES-256-GCM
    let sealed = key.seal("card 4242".bytes(), context: "user 7".bytes())
    let opened = key.open(sealed, context: "user 7".bytes())?
    console.println("{String.fromBytes(opened).unwrapOr("")}")

    let signer = crypto.PrivateKey.generate()                // Ed25519
    let signature = signer.sign("release 1.2".bytes())
    console.println("{signer.publicKey().verify("release 1.2".bytes(), signature)}")
}
```

- Nothing asks for a nonce, an IV or a mode: `seal` makes its nonce and
  `open` finds any change. Keys have no `Display`, so none reaches a log.
- `verifyPassword` also reads the bcrypt, PBKDF2 and Argon2 hashes other
  programs stored. `==` on bytes and digests takes the same time wherever
  they differ: comparing an HMAC with `==` is safe.
- `PrivateKey.fromPem` and `PublicKey.fromPem` read what OpenSSL writes;
  `crypto.certificates(pem)` reads certificates.

## Trees of files

```tondo
import std.console
import std.fs
import std.gzip
import std.path

fn main(): !fs.FsError {
    for entry in fs.walk("content")? {             // each directory before what it holds
        console.println("{entry.path}")
    }
    for page in fs.glob("content/**/*.{{md,markdown}}")? {
        let packed = gzip.compress(fs.readBytes(page)?)
        let name = path.relative("content", page).unwrapOr(page)
        fs.writeBytes(path.join("public", "{name}.gz"), packed)?
    }
    let work = fs.createTemporaryDirectory()?      // the program removes it
    console.println(work)
    for changes in fs.watch(["content"])? {        // a batch once they settle
        console.println("{changes.length()} changed")
    }
}
```

A `fs.FileSystem` is a tree with the same operations on paths from its top:
`fs.disk("site")`, `fs.memory()`, `embed.files("theme")`, an archive's,
`tree.sub("content")`, or `fs.overlay(site, theme)`, which reads from the
first layer that has a path. `app.files`, `set.addFiles` and
`archive.create` take one. `archive.create("site.zip", fs.disk("public"))`
and `archive.open(path)?.extract(into)` write and read zip and tar, which
an archive whose paths would leave `into` cannot use.

`path.matches(pattern, path)` is the glob of `fs.glob`: `*`, `?`, `[a-z]`,
`{a,b}` and `**`. `fs.metadata` gives `modified`, `permissions`, `inode`,
`links`, `owner` and `group`; `fs.setPermissions(path, 0o755)`,
`fs.setModified`, `fs.createSymlink`, `fs.createLink` (a hard link),
`fs.readSymlink`, `fs.realPath` and `fs.space` do the rest. `file.lock()`
waits for a file's lock and `file.tryLock()` does not; closing the file lets
it go. `gzip.writer(out)` and `gzip.reader(input)`
compress a stream of `std.io`, and `std.zstd` has the same functions. Their
`close()` closes them and lets go of the stream under them, which closes with
its last copy: a file the writer alone holds closes with it, and
`console.stdout()` stays open.

## Images

```tondo
import std.image

fn main(): !image.ImageError {
    let photo = image.open("photo.jpg")?              // turned as its EXIF says
    photo.fit(800, 600).save("photo-800.webp")?       // fits inside, never grows
    photo.fill(200, 200, anchor: image.Anchor.Top).save("avatar.jpg", quality: 80)?
    let size = image.info(photo.encode(image.Format.Png)?)?
    let wide = photo.resize(width: 1200)              // the height keeps the proportions
}
```

An image never changes: each operation gives a new one. `crop`, `rotate`
by multiples of 90, `flipHorizontal`, `grayscale`, `blur`, `overlay` for a
watermark, and `pixel` for a color; `image.blank(w, h, image.Color.hex("#ffffff")?)`
makes one. `save` picks the format by the extension.

## Dates in time zones

`time.DateTime` is always UTC; a `time.ZonedDateTime` is what the clocks of
a zone show, with their offset:

```tondo
import std.console
import std.time

fn main(): !time.TimeError {
    let madrid = time.zone("Europe/Madrid")?
    let now = time.utcNow().inZone(madrid)
    console.println("{now}")                                        // 2026-10-29T13:05:00+01:00
    console.println(now.format("%-d de %B de %Y, %H:%M", locale: "es"))
    let meeting = madrid.at(year: 2026, month: 11, day: 2, hour: 9)?
    console.println("{meeting.utc()}")                              // 2026-11-02T08:00:00Z
    let read = time.parseZoned("02/11/2026 09:00", madrid, pattern: "%d/%m/%Y %H:%M")?
    console.println("{read == meeting} {time.parseDuration("1h30m")?}")       // true 1h30m0s
}
```

Patterns are `strftime`'s, `%Y-%m-%d %H:%M:%S`, and a literal one with a
directive that does not exist is a compile error. Names come from CLDR in
the language `locale` gives, never the system's. Zones come from the
system's tz database, and those it lacks from the copy in `tondo`;
`TONDO_ZONES=builtin` uses that copy always.

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

A logger put in `log.Context` is the one `log.info(...)` and the other
functions of `std.log` write with, everywhere below: `with log.Context {
logger: some(log.Logger.new(format: log.Format.Json)) }` at the start of
`main`.

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

A program profiles itself with `std.profile`, as a continuous profiler
does: `let cpu = profile.startCpu()`, then `cpu.stop().pprof()`, the bytes
of a pprof file; `profile.startMemory()` and `profile.memory()`;
`profile.tasks()`; `profile.startBlocking()` and `profile.blocking()`, where
tasks wait for others; `profile.startContention()` and
`profile.contention()`, where locks others waited for are let go.
`profile.withLabels(["endpoint": path], () { handle(request) })` labels the
samples of its work, and of the tasks it starts, so a profile divides the
time by them; `let previous = profile.setLabels(...)` and
`defer previous.restore()` do it when start and end are different calls.
`diagnostics.metrics()` gives the counters `tondo counters` shows, to send
where the program wants, and `diagnostics.stack()` the frames of the task
that asks.

`tondo profile -- app.to` (or `tondo profile app --duration 10`) writes
`app-<pid>.cpu.pprof`, a profile with the function and line of every frame
for `go tool pprof`, speedscope or Datadog; `--memory` profiles what it
allocates and holds, `--tasks` where its tasks wait. `tondo profile show
<file>` gives the functions that take the most, `--lines` the program's
lines with their code, `--html flame.html` a flame graph, `--json` data:

```text
     flat  flat%       cum   cum%  line
     94ms  64.4%      94ms  64.4%  busy.to:14 in words  total += text.split(" ").length()
     45ms  30.8%      45ms  30.8%  busy.to:13 in words  let text = "word {i} and more words"
```

`tondo dump app` writes `app-<pid>.dump.json`: every task, where it waits or
runs, where it was spawned, the locks it holds and the fields of its
contexts a `with` set, the counters, and the
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
break app.to:12 hits 5          the 5th time (or >= 5, % 5)
break app.to:12 log at {i}      write it there, and go on
continue | step | next | finish run, into calls, over them, out
locals | print user.name        the bindings, or a part of one: items[2], map["k"]
children user | stack | frame 1 a value's parts, the calls, a caller's bindings
context                         the task's contexts, and the `with` that set each field
set count 3 | catch none        change a binding; do not stop at panics
tasks | task 2 | pause | delete 1 | quit
```

A panic that no `recover` stops stops the debugger where the program's code
panicked, before it unwinds. When
one task stops, all stop at their next line, and go on together.
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
move, their parameters included, and what goes back to their callers, a
result or a value lent, converts back, field by field, a new field
starting empty (0, `""`, `[]`, `none`) and a number of another numeric
type converting as `T(x)` does, and a field, a variant or a type renamed
keeping its values; reload says what it did to them. Closures follow too,
when another closure comes before them or what they capture changes shape.
What a `Ref`, a channel or a `Mutex` holds converts once, in place, while
every task stops at a statement: a task waiting for a channel, a lock or a
line starts that statement again with the new code. Reload refuses, and
`--watch` starts the program again, when a statement may still use what
they hold with the old code after a wait it cannot end, as reading a file,
or when a task holds the lock; write such a wait in a `let` statement of its
own. A value kept in a task or a `std.sync` collection does not convert.
Old code is never freed, and a call that moves leaves its old frame below
it: each reload takes a little memory, and each move one frame of stack,
which only reloading in the middle of very deep recursion notices.
Under `tondo debug --live app.to`, `reload` works where the program
stopped: edit the function, reload, continue; or `restart`, which runs the
stopped function again from its first line, with its new code.
