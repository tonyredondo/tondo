<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/branding/tondo-wordmark-dark.svg">
    <img src="docs/branding/tondo-wordmark.svg" alt="Tondo" width="250">
  </picture>
</p>

<p align="center">
  <strong>Small by design, complete in practice.</strong><br>
  A statically typed language that compiles to native executables,<br>
  with its whole toolchain in one binary.
</p>

<p align="center">
  <a href="https://github.com/tonyredondo/tondo/actions/workflows/ci.yml"><img src="https://github.com/tonyredondo/tondo/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI"></a>
  <img src="https://img.shields.io/badge/runs%20on-Linux%20%7C%20macOS-7c3aed" alt="Runs on Linux and macOS">
  <img src="https://img.shields.io/badge/arch-x86--64%20%7C%20arm64-7c3aed" alt="x86-64 and arm64">
  <img src="https://img.shields.io/badge/status-pre--release-f59e0b" alt="Status: pre-release">
</p>

<p align="center">
  <a href="#why-tondo">Why Tondo</a> ·
  <a href="docs/tour.md">A tour of the language</a> ·
  <a href="docs/language/README.md">The manual</a> ·
  <a href="docs/toolchain.md">The toolchain</a> ·
  <a href="examples/README.md">Examples</a> ·
  <a href="docs/comparison.md">Next to Go, Rust and C#</a> ·
  <a href="#getting-started">Getting started</a>
</p>

---

Tondo is meant to be light to write, with little to annotate and errors that
say what to type, and complete when a program grows. One executable, `tondo`,
is the compiler, the standard library, the test runner, the formatter, the
debugger, the profiler, the language server and an MCP server for coding
agents.

```tondo
import std.http

pub type User = {
    id: Int
    name: String
}

fn find(id: Int): User? {
    if id == 1 { some(User { id: id, name: "Ada" }) } else { none }
}

fn main(): !http.HttpError {
    var app = http.App()
    app.get("/users/:id", (id: Int) { find(id) })
    app.listen(8080)?
}
```

`tondo run server.to` starts it. The route's `id` arrives as an `Int`, the
record is written as JSON, and `none` is a 404. The HTTP server, the JSON
encoder and the linker that made the executable all came with `tondo`.

> [!NOTE]
> Tondo is in early development: nothing is released yet and the language can
> still change. It runs on Linux and macOS, on x86-64 and arm64.
> [Status](#status) says what is done and what is not.

## Why Tondo

<table>
<tr>
<td width="50%" valign="top">

**Errors are in the signature**

`T ! E` says a function can fail, `?` passes the failure on, and a result
nobody looks at does not compile. There is no `null` and there are no
exceptions: what can be missing is a `T?`.

</td>
<td width="50%" valign="top">

**Any function can wait**

There is no `async` and no `await`. `spawn` starts a task, a call that waits
is written like any other, and tasks run on all cores. A task belongs to the
block that started it, so none is left behind by accident.

</td>
</tr>
<tr>
<td valign="top">

**Values behave like values**

Assigning a record, an array or a map copies it, as far as anyone can tell:
the storage is shared until one of them changes. Memory is managed for you,
cycles included, and safe code has no data races.

</td>
<td valign="top">

**Mistakes say what to type**

Every error has a code and says how to fix it, often with the code to
write. `tondo fix` applies the repairs the compiler is sure of, and
`tondo check --json` gives them to a tool as edits.

</td>
</tr>
<tr>
<td valign="top">

**One binary is the toolchain**

Tests, coverage, benchmarks, fuzzing, a debugger that steps back, dumps,
live reloading and profiles are commands of `tondo`. It links what it
compiles too, so building a program needs no C toolchain.

</td>
<td valign="top">

**There is one way to write it**

One loop (`for`), one way to tell cases apart (`match`), one format:
`tondo fmt` has no style to choose. `tondo guide` teaches the language a
topic at a time, to people and to models, and `tondo api` shows what a
module offers.

</td>
</tr>
</table>

## A look at the language

A function that can fail says so, and says what was going on when it did:

```tondo
import std.console
import std.fs
import std.json

pub type Config = { name: String, port: Int = 8080 }

fn load(path: String): Config ! Failure {
    let text = fs.readText(path).context("reading {path}")?
    json.decode[Config](text).context("decoding {path}")?
}

fn main() {
    match load("app.json") {
        ok(config) => console.println("{config.name} listens on {config.port}")
        err(failure) => console.println("{failure}")
    }
}
```

Without the file it prints `reading app.json: not found`. With it, `Config`
is read from the JSON with nothing to derive or register, and a document
without a `port` gets `8080`.

Concurrent code reads like the rest. Here two pages are fetched at once, the
first error cancels the other task, and the whole block has five seconds:

```tondo
import std.console
import std.http
import std.time

fn size(url: String): Int ! http.HttpError {
    http.get(url)?.text()?.length()
}

fn main() {
    var pages = Group[Int ! http.HttpError]()
    for url in ["https://example.com", "https://example.org"] {
        pages.add(spawn size(url))
    }
    match within time.seconds(5) { pages.wait() } {
        ok(sizes) => console.println("{sizes}")
        err(error) => console.println("gave up: {error}")
    }
}
```

And when the code is wrong, the compiler answers with what to write instead.
These are two habits from other languages:

```text
error[E1001]: `String` has no method `upper`: use `toUpper()`
 --> hello.to:5:26
  |
5 |     console.println(name.upper())
  |                          ^^^^^
error[E1102]: operator `+` cannot be applied to `String`: use interpolation, `"{a}{b}"`, or `a.concat(b)`
 --> hello.to:7:25
  |
7 |         console.println("long: " + name)
  |                         ^^^^^^^^^^^^^^^
```

[A tour of the language](docs/tour.md) goes through the rest: records, enums
and exhaustive `match`, generics and traits, slices and arithmetic on arrays,
channels and `select`, contexts that follow the work instead of being passed
as parameters, exact decimals, reflection, and calling C.

## One binary, the whole toolchain

| Area | What `tondo` does |
| --- | --- |
| Writing | `check` with a repair in every diagnostic, `fix`, `fmt`, `explain <code>`, `api` to see a module without its bodies, `guide` |
| Building | Native executables, static ones, builds for another architecture, libraries for C with their header, and its own linker |
| Testing | Unit and integration tests in parallel, coverage of lines and branches, snapshots, only the tests a change reaches, `--watch` |
| Tasks in every order | A test that starts tasks runs again in every order of them that differs, and a failure gives the order to repeat and debug |
| Designs | `std.model` goes through every state of a design or of the code itself and says which step breaks what must hold |
| Benchmarks and fuzzing | Benchmarks compared with a commit, with allocations and instructions; fuzzing that knows the type of its input and keeps what failed as tests |
| Debugging | Breakpoints that stop one task of a running Release build, record and replay, stepping back, and asking why a value is what it is |
| Failures | A crash leaves a dump with what every task held; dumps group by failure, compare, and turn into a test that fails the same way |
| Live programs | A running program takes in the code you save and keeps its state; a panic waits for your fix |
| Performance | CPU, memory and wait profiles with lines, flame graphs, what a release build inlined or left on the heap, and the machine code of a function |
| Editors | A VS Code extension with all of this, and `tondo lsp` and `tondo dap` for any other editor |
| Coding agents | `tondo mcp` gives every command as a tool, with runbooks for what to do when something goes wrong |

[The toolchain](docs/toolchain.md) shows each of them with its commands.
One example: this test passes almost every time it runs.

```tondo test
import std.sync

fn deposit(balance: sync.Mutex[Int], amount: Int) {
    let seen = balance.get()
    balance.set(seen + amount)
}

test depositsAddUp {
    let balance = sync.Mutex(0)
    let first = spawn deposit(balance, 10)
    let second = spawn deposit(balance, 5)
    first.wait()
    second.wait()
    assert(balance.get() == 15)
}
```

The compiler warns about `deposit` (`W1016`: another task may write between
the read and the write), and a plain `tondo test` goes through the orders of
the two tasks, finds the one that loses a deposit, and says how to see it
again:

```text
test bank::unit::bank::depositsAddUp ... ok
bank::unit::bank::depositsAddUp: failed in order 1
again in that order: tondo test --exact bank::unit::bank::depositsAddUp --schedules at:0*15
debug it in that order: tondo debug --test bank::unit::bank::depositsAddUp --schedule at:0*15

panic[P0007]: assertion-failed: balance.get() == 15
  left:  5
  right: 15
```

## A standard library for whole programs

Every module comes with `tondo`, and `tondo api <module>` shows it with an
example that compiles for each of its types.

| Area | Modules |
| --- | --- |
| Web | `std.http`: clients and servers for HTTP/1.1 and HTTP/2, typed handlers, OpenAPI, WebSockets, Server-Sent Events. `std.grpc`, `std.graphql`, `std.template`, `std.url`, and `std.acme` for Let's Encrypt certificates a server renews by itself |
| Data | `std.json`, `std.yaml`, `std.toml`, `std.xml`, `std.csv`, `std.cbor`, `std.messagepack`, `std.protobuf`: the compiler writes the encoder and decoder of every type, once for all formats (`std.serialization`). `std.validate` holds what is read to a type's rules |
| Databases | `std.sql`: SQLite, PostgreSQL and MySQL through one API, with queries whose values are parameters, transactions and migrations |
| Network and security | `std.net` (TCP, UDP, Unix sockets, TLS), `std.crypto`, `std.hash`, `std.uuid`, `std.random` |
| Files and the system | `std.fs`, `std.path`, `std.io`, `std.archive`, `std.gzip`, `std.zstd`, `std.embed`, `std.image`, `std.process` with pipelines, `std.env` |
| Text, numbers and time | `std.regex`, `std.text`, `std.format`, `std.encoding`, `std.math` with `BigInt` and `Decimal`, `std.time` with zones and calendars, `std.collections` |
| Concurrency | `spawn`, `Group`, channels and `select` in the language; `std.sync` (locks, atomics, shared collections, rate limits, retries) and `std.executor` (pools and actors) |
| Programs | `std.cli` (commands from types, tables, progress bars), `std.console`, `std.log`, `std.mcp` |
| The program itself | `std.testing`, `std.model`, `std.profile`, `std.diagnostics`, `std.debug`, `std.reflect`, `std.meta`, `std.ffi` |

## Speed

Tondo compiles to native code with Cranelift, of which it keeps its own copy
for the optimizations it adds. The goal is every benchmark of
[`bench/`](bench/README.md) at or under Go's time and allocations. Measured
on 2026-11-12 at `ab5eac89`, on an AMD Ryzen 9 5950X against Go 1.27, 33 of
the 36 are; `loops/collatz`, `tasks/spawn` and `floats/sqrt` take at most
1.07× Go's time, and `net/tls echo` and `http/tls` make more allocations. On
2026-10-26, with the 27 benchmarks there were then, 23 of the 27 were slower
than Go.

| Benchmark | Tondo | Go | Tondo/Go |
| --- | ---: | ---: | ---: |
| arrays/sort 10000 | 66.85 µs | 354.46 µs | 0.19× |
| regex/match 1000 | 38.09 µs | 149.22 µs | 0.26× |
| maps/insert 10000 ints | 135.64 µs | 490.74 µs | 0.28× |
| structs/users 1000 | 19.28 µs | 38.12 µs | 0.51× |
| json/parse object | 2.28 µs | 3.62 µs | 0.63× |
| tasks/channel 10000 | 191.76 µs | 273.01 µs | 0.70× |
| runtime/select 1000 | 124.60 µs | 159.16 µs | 0.78× |
| calls/fib 25 | 232.38 µs | 290.51 µs | 0.80× |
| loops/sum 1000000 | 612.29 µs | 724.40 µs | 0.85× |
| runtime/mutex 4x1000 | 20.43 µs | 21.84 µs | 0.94× |
| calls/closure 1000 | 205.8 ns | 207.5 ns | 0.99× |
| tasks/spawn 1000 | 179.61 µs | 171.83 µs | 1.05× |
| loops/collatz 10000 | 905.83 µs | 845.91 µs | 1.07× |

[`bench/README.md`](bench/README.md) has every row with its allocations, and
how to run them. The toolchain says where the time goes: `tondo optimizations`
lists what a release build inlined, kept off the heap and still checks,
`tondo asm` shows a function's machine code under its lines, and
`tondo test --bench --against main --max-regression 5` fails a change that
makes a benchmark slower.

## Next to Go, Rust and C#

Tondo takes Go's small core, tasks and tooling, Rust's `Option`, `Result`,
enums and `match`, and from C# the ways of lending a value for the length of
a call: `ref` to read it and `mut` to change it.

| | Tondo | Go | Rust | C# |
| --- | --- | --- | --- | --- |
| A function fails | `T ! E` and `?` | `error` and `if err != nil` | `Result<T, E>` and `?` | exceptions |
| A missing value | `T?`, no `null` | `nil` | `Option<T>` | `null` |
| Waiting | any function, unmarked | any function, unmarked | `async` and `.await` | `async` and `await` |
| A task nobody waits for | is waited for when its block ends | keeps running | depends on the runtime | keeps running |
| Memory | automatic: counts and a collector of cycles | automatic: a tracing collector | ownership and lifetimes | automatic: a tracing collector |
| Assigning a collection | copies it | shares it | moves it | shares it |
| Polymorphism | traits, resolved when compiling | interfaces, satisfied implicitly | traits, static or `dyn` | classes and interfaces |
| What you run | a native executable | a native executable | a native executable | a runtime, or Native AOT |

[Tondo next to other languages](docs/comparison.md) goes through what is
shared with each, what was left out and why, with the same code in both.
Each of the three has a page that writes twelve small programs in it and in
Tondo, from copying a list to an HTTP server, and so have
[Python](docs/comparison/python.md),
[TypeScript](docs/comparison/typescript.md),
[Java](docs/comparison/java.md), [Kotlin](docs/comparison/kotlin.md) and
[Swift](docs/comparison/swift.md).

## Getting started

```sh
git clone https://github.com/tonyredondo/tondo && cd tondo
sh dist/install.sh
```

The script builds `tondo` with Rust (from [rustup](https://rustup.rs)) and a C
compiler; without Rust, it downloads the latest build that passed CI, with
GitHub's `gh`. Then it runs `tondo setup`, which checks that `tondo` compiles
and runs a program and sets it up for what it finds: the Tondo skill and the
`tondo mcp` server for Claude Code and Codex, and the extension of
[`editors/vscode`](editors/vscode) for VS Code, Cursor, VSCodium and
Windsurf.

```sh
tondo new shop        # a project: tondo.toml, src/main.to and a test
tondo run shop        # compile and run it
tondo test shop       # run its tests
tondo guide           # learn the language, a topic at a time
```

In the editor, *Tondo: Get Started* walks through a first project. With a
coding agent, open this repository in it and ask it to set Tondo up:
[`AGENTS.md`](AGENTS.md) tells it how, and how to check that it works. Other
editors use `tondo lsp`, and `tondo guide projects` says how to set one up.

<details>
<summary><strong>Without the script, and without Rust</strong></summary>

Every CI run leaves a ready `tondo` for each platform as an artifact,
`tondo-<target>`: x86-64 and arm64 Linux with glibc 2.34 or later, musl for
older systems, and macOS on Apple silicon and Intel. Its `bin/tondo` is the
whole toolchain, and it links executables itself
([`dist/README.md`](dist/README.md)). The same run leaves the extension's
`.vsix`.

Building from source requires Rust, whose version and musl targets
`rust-toolchain.toml` pins, and a C toolchain, with which Rust links the
compiler:

```sh
cargo build --release --locked -p tondo
cp target/release/tondo ~/.cargo/bin/
tondo setup
```

</details>

### Every day

```text
tondo run program.to -- args      compile (cached) and run
tondo program.to args             the same for a script: #!/usr/bin/env tondo works
tondo build --output app          a native executable; --static, --target aarch64-linux
tondo check                       diagnostics only; --json for tools
tondo fix                         apply the compiler's safe repairs
tondo fmt program.to              the canonical format; --check to verify
tondo test                        the tests; --coverage, --bench, --fuzz, --watch
tondo debug program.to            break, step, look at every task
tondo api std.http                what a module offers, with examples
tondo explain E1102               what an error or a panic code means
```

`tondo help` lists the commands by group, `tondo help <group>` says what
each of a group does, and `tondo <command> --help` explains one;
`tondo completions bash|zsh|fish` completes them in a shell. With `--json`,
every command writes its result as one JSON object for tools.

`tondo run` works like `go run`: it compiles the program to a native
executable, caches it under `~/.cache/tondo`, and executes it. There is no
interpreter, so `run` and `build` always agree.

### Projects

A program can be one file or a project: a directory with `src/main.to` (or
`main.to` at its root), where each directory is a module and every module of
the package is imported by its path. Without a path, the commands use the
project in the current directory.

```text
app/
  src/
    main.to            import app.models, import http.client
    models/user.to     pub type User = { ... }
  tondo.toml           optional, see below
  tondo.lock           written by tondo: the exact version of each fetched dependency
```

```toml
[package]
name = "app"

[dependencies]
units = { path = "libs/units" }
http = { git = "https://github.com/acme/http", tag = "v1.2.0" }
zlib = { url = "https://example.com/zlib-1.0.tar.gz" }
```

`tondo add` and `tondo remove` edit the dependencies. They are fetched with
the system's `git`, `curl` and `tar` into `~/.cache/tondo` and pinned in
`tondo.lock`; `tondo update` fetches them again.

## Status

Nothing is released yet, and the language can still change. What each
milestone covers is in [`docs/roadmap.md`](docs/roadmap.md).

| Area | State |
| --- | --- |
| Language: types, generics, traits, ownership, closures, tasks, contexts | Done |
| Native compiler and linker, on all cores; cross builds | Done |
| Linux x86-64 and arm64, glibc 2.34 or later, and musl for anything older | Done |
| macOS on Apple silicon and Intel, signed and notarized | Done |
| Standard library, measured against what porting Gin, Hugo and dd-trace-go needs | Done |
| Tests, every order of their tasks, benchmarks, fuzzing, coverage, profiling | Done |
| Debugger, dumps, live builds, language server, VS Code, MCP | Done |
| Windows | Not started |
| SIMD API | Accepted, not started |
| Profile-guided optimization while a program runs | Research |
| A first release | Not yet |

Known limits today: logpoints and Release breakpoints work on Linux only,
since macOS does not let a program change its code.

## Documentation

| Read | For |
| --- | --- |
| [A tour of the language](docs/tour.md) | The language by example, from a script to tasks and C |
| [The language](docs/language/README.md) | The manual: the whole language in twenty chapters, with programs and what they print |
| [The standard library](docs/std/README.md) | A page for each module, with every declaration, its documentation and examples |
| [The toolchain](docs/toolchain.md) | What each command of `tondo` is for, and a chapter for each part of it at work |
| [Examples](examples/README.md) | Programs of the size one meets at work: an API over a database, a gRPC service, a chat, a library for C, scripts |
| [Tondo next to other languages](docs/comparison.md) | What it shares with Go, Rust and C# and where it differs, and the same programs in eight languages |
| `tondo guide`, in [`crates/tondo/guide`](crates/tondo/guide/overview.md) | The reference people and models learn from, a topic at a time |
| [`spec/`](spec) | The specifications that define the language, its library, its tests and its toolchain |
| [`docs/`](docs/README.md) | Everything else: the roadmap, the decisions of design and the plans |

## Working on Tondo

| Path | Contents |
| --- | --- |
| `crates/tondo` | Compiler and CLI: lexer, parser, checker (AST to typed HIR), MIR lowering with explicit reference counting, Cranelift code generation, linking |
| `crates/tondo-runtime` | Runtime linked into every executable. The compiler's build script compiles it as a static library and embeds it |
| `tests/cases` | Tondo programs whose comments state the expected output, exit status, panic or diagnostic |
| `tests/projects` | Projects of several modules, with expectations in the same comments |
| `spec` | Language, standard library, testing and toolchain specifications |
| `docs/decisions.md` | Short records of design decisions |
| `bench` | The same work in Tondo and in Go, measured side by side |
| `editors/vscode` | The VS Code extension, in TypeScript: `npm test` runs its tests, `npm run package` makes the `.vsix` |

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The case runner executes every program with `TONDO_RT_CHECK_LEAKS=1`, so the
tests also fail on leaked or doubly freed strings. The examples of this
README are compiled by the tests too. [`AGENTS.md`](AGENTS.md) has the rest:
the checks before a commit, what a feature lands with, and how to see what
the compiler does.

- Keep it small: a feature lands with its tests and nothing else.
- Specification changes are deliberate and reviewed; the implementation does
  not redefine the language.
- Tests are ordinary `cargo test` plus Tondo programs with stated
  expectations. There is no separate evidence ledger.

The previous implementation (bytecode VM first) is archived at
[tonyredondo/tondo-legacy](https://github.com/tonyredondo/tondo-legacy). It
remains useful as a reference and as an oracle for differential testing.
