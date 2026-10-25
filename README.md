# Tondo

Tondo is a statically typed, compiled language with automatic memory
management: *small by design, complete in practice*. The normative definition
lives in [`spec/`](spec/); this repository is a fresh, native-first
implementation of it.

**Status:** early development; nothing is released yet. It compiles to
native executables with [Cranelift](https://cranelift.dev), on Linux and
macOS, and the roadmap below says what works.

```text
$ cat hello.to
import std.console

fn main() {
    console.println("Hello, world")
}
$ tondo run hello.to
Hello, world
```

## Getting started

```sh
git clone https://github.com/tonyredondo/tondo && cd tondo
sh dist/install.sh
```

The script builds `tondo` with Rust (from [rustup](https://rustup.rs)) and
a C compiler, or without Rust downloads the latest build that passed CI
with GitHub's `gh`, and installs it. Then it runs `tondo setup`, which
checks that it compiles and runs a program, and sets it up for the coding
agents it finds: the Tondo skill for Claude Code and Codex, and
`tondo mcp`, an MCP server whose tools do everything `tondo` does,
registered with both. Then `tondo guide` teaches the language.

With an agent, open this repository in it and ask it to set Tondo up:
[`AGENTS.md`](AGENTS.md) tells it how, and how to check that it works.
Editors use `tondo lsp`; `tondo guide projects` says how to set one up.

## Using it

Every CI run leaves a ready `tondo` for each platform as an artifact,
`tondo-<target>` (x86-64 and arm64 Linux with glibc and musl, macOS on Apple
silicon and Intel): `bin/tondo` is the whole toolchain, and it links
executables itself, with no C toolchain (`dist/README.md`).

Building requires Rust (`rust-toolchain.toml` pins the version and the musl
targets `--static` needs) and a C toolchain, with which Rust links the
compiler.

```text
cargo build --release
target/release/tondo run program.to -- args      # compile (cached) and run
target/release/tondo build program.to --output app
target/release/tondo check program.to            # --json for structured diagnostics
target/release/tondo fmt program.to               # canonical format; --check to verify
target/release/tondo script.to args              # for #!/usr/bin/env tondo
```

`tondo run` works like `go run`: it compiles the program to a native
executable, caches it under `~/.cache/tondo`, and executes it. There is no
interpreter, so `run` and `build` always agree.

A program can also be a project: a directory with `src/main.to` (or
`main.to` at its root), where each directory is a module and every module
of the package is imported by its path. Without a path, the commands use the
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

Dependencies are fetched with the system's `git`, `curl` and `tar` into
`~/.cache/tondo` and pinned in `tondo.lock`; `tondo update` fetches them
again.

## How it is built

| Path | Contents |
|---|---|
| `crates/tondo` | Compiler and CLI: lexer, parser, checker (AST to typed HIR), MIR lowering with explicit reference counting, Cranelift code generation, linking |
| `crates/tondo-runtime` | Runtime linked into every executable (strings, console, panics). The compiler's build script compiles it as a static library and embeds it |
| `tests/cases` | Tondo programs whose comments state the expected output, exit status, panic or diagnostic |
| `tests/projects` | Projects of several modules, with expectations in the same comments |
| `spec` | Language, standard library, testing, toolchain and LLM-form specifications |
| `docs/decisions.md` | Short records of design decisions |

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The case runner executes every program with `TONDO_RT_CHECK_LEAKS=1`, so the
tests also fail on leaked or doubly freed strings.

## Roadmap

Every milestone ends with programs that compile to native executables and run.
`tests/cases` shows what works today.

| Milestone | Scope | State |
|---|---|---|
| **H0** | Integers with checked arithmetic, `Bool`, `String` with interpolation, functions, `let`/`var`, `if`, `for` (infinite, conditional, integer ranges), `break`/`continue`, `return`, `panic`/`assert`, `std.console.print`/`println`, root scripts, native codegen and linking | Done |
| **H1** | Data: tuples, records, newtypes, aliases, enums (including recursive ones), `match` with exhaustiveness, patterns (including array patterns), `Option`/`Result` with `ok`/`err`/`some`/`none`, `?` and `fail`, fallible `main`, structural unions and error unions, `Char`, `Byte`, floats, numeric conversions, string indexing and slicing, `Array`/`Deque`/`Map`/`Set` with copy-on-write, array arithmetic and slice assignment | Done |
| **H2** | Abstraction: methods, named arguments, variadics, generics (monomorphization), traits with static dispatch, closures and function values, iterators, opaque `impl` results | Done |
| **H3** | Ownership: affine values (`~Copy`), `ref`/`mut`/`var` loans, region loans, loan patterns, loan modes in function types, `defer`, panic cleanups | Done |
| **H4** | Projects: modules and packages, `tondo.toml`, dependencies (path, git, archives) and `tondo.lock`, canonical formatter (`tondo fmt`), JSON diagnostics | Done |
| **H5** | Concurrency without colors: stackful tasks on all cores, `spawn`, `Join`, `Group`, channels, `select`, timeouts, cancellation | Done |
| **H6** | Hosted standard library: shared state (`std.sync`), streams (`std.io`), filesystem and paths, processes and pipelines, environment, time, JSON with automatic encoding, regular expressions; fire-and-forget tasks; parameters with default values | Done |
| **H7** | `unsafe` functions, blocks, closures and function types; `Pointer[T]` with its raw operations; calling C: `extern fn` (with C's `...`), `extern type` structs by value and by pointer, callbacks into Tondo from any thread, `std.ffi`, native libraries; lossless numeric widening | Done |
| **Targets** | Linux x86-64 and arm64 (glibc and musl), macOS on Apple silicon and Intel, each with its C calling convention; CI on all of them | Done |
| **Bindings** | `tondo bind` writes a module of `extern` declarations and constants from C headers, a `pkg-config` package, or a library's debug information; unions, `va_list`, macros, bit fields; static and shared native libraries; `const` | Done |
| **H8** | Testing: `test` and `suite`, unit test companions and integration test roots, `std.testing` (`log`, `tags`, `failNow`, `skip`), `tondo test` running tests in worker processes with timeouts, selection by filter, glob or identity, shards, random order with a seed, retries and flaky passes, repeats, CODEOWNERS, JSON and JUnit reports, attachments, snapshots of any value in a file each, virtual time | Done |
| **Stdlib 1** | The prelude: the everyday methods of `String` (`split`, `lines`, `repeat`, padding, trimming, Unicode case, parsing numbers), `Char`, `Int`, `Float`, `Option`, `Result`, `Array` (`map`, `filter`, `reduce`, `sorted`, `join`, `groupBy`, ...), `Map`, `Set` and lazy `Iterator` adapters; `Ordered` and `Numeric`; `std.math` | Done |
| **H9** | Runtime reflection (`std.reflect`: get, set, call, conversion by shape and type descriptions), `derive` with providers and generators declared in `tondo.toml`, both built for the `meta` profile, with `tondo expand` | Done |
| **Stdlib 2** | Ranges as values and lazy `String.chars()` and `Array.iter()`, so that `(0..10).map(...)` and `text.chars().filter(...)` work; `std.format` (a text builder), `std.encoding` (Base64, hex, URLs), `std.hash` (SHA-1, SHA-256, SHA-512, HMAC), `std.random` (seeded generators, shuffle, choice, entropy), `std.uuid` (v4, v5, v7), ISO 8601 dates in `std.time`, and a failing `main` that shows its error's `Display` | Done |
| **Stdlib 3** | `std.csv` (streaming and whole), `std.log` (levels, fields, text or JSON lines), and `std.http`: a client for HTTP/1.1 and HTTP/2 over TLS with streaming bodies, multipart, Server-Sent Events and WebSockets; a server with routes, middleware, typed handlers, OpenAPI, files with resumable downloads, resumable uploads, Server-Sent Events, WebSockets and TLS | Done |
| **Stdlib 4** | One serialization protocol for every format (`std.serialization`): events, one error type with path, line and column, and `Encode`/`Decode` that the compiler implements for every type, generic over each format's reader and writer; JSON on top of it; `std.toml` (TOML 1.1, with dates); `std.yaml` (a safe subset of YAML 1.2, many documents, bounded aliases); `std.messagepack` (shortest forms, bytes and timestamps native); `std.cbor` (RFC 8949, preferred and deterministic serialization); `std.net` (TCP with TLS, listeners, UDP and names, a task per connection without a thread per connection, a timeout per call). | Done |
| **H10** | Tooling for agents: `tondo guide` teaches the language, its library and its commands a topic at a time, with examples that compile and the mistakes models make, and can install itself as an agent skill; `tondo api` shows a module's declarations and documentation without bodies, about a quarter of its tokens, for the standard library, the project and its dependencies; `tondo fmt --compact` prints Tondo with about 15% fewer tokens. A separate LLM dialect was measured on seven tokenizers and dropped: it saved nothing that compact Tondo does not | Done |
| **H11** | Diagnostics and editors: aborts, illegal instructions, arithmetic faults and faults inside C report their trace; `tondo test --schedules N` runs each test in many orders of its tasks, again with `--seed`; a test that leaves tasks running fails, saying where they wait; `tondo lsp`, a language server with diagnostics, hover, definitions, references, rename, completion, semantic highlighting, signatures, inlay types, symbols, formatting and code actions from the compiler's repairs; functions compile on every core; workers poll the network themselves, for Go's latency on a request and its answer. A cache of compiled functions between builds waits until a build is slow enough to need it | Done |
| **std.executor** | Pools that bound how many jobs run and how many wait, whose jobs are ordinary tasks with their `Join`; actors that own state behind a bounded mailbox, stopped by their owner or by the end of its block. A closure that cannot fail fits a parameter that may, `fn(...): !E`, with `E` inferred as `Never` | Done |
| **Identity and cycles** | `Ref[T]`, values with an identity that compare and hash by it, read in place and never changed; a collector of the cycles of references that counting alone would keep, which starts from the shared state every cycle passes through and stops the workers briefly at safe points; `WeakRef[T]` for caches | Done |
| **Warnings** | The `core` profile of 22.8 for the program's own code: unused imports, bindings and parameters (`_name` holds a value to the end of its scope), names against the conventions with the conventional one, confusable identifiers by their Unicode skeletons, redundant conversions, comparisons with NaN, repeated `Set[...]` entries, unreachable code, serial spawns in loops | Done |
| **Interruption** | Ctrl-C stops `tondo test`: no more tests start, the running ones are cancelled and run their `defer`s, a worker that does not stop is killed, and the run exits with `4` (`3` if it had to kill) without reports | Done |
| **Examples of documents** | `tondo doc-test` checks the examples of a Markdown document as 21.6 says, as syntax, as fragments and scripts against the fixtures of Appendix C, or as failing with the errors they name, and records each as JSON; closures that give a capture away are called once (`CallOnce`) | Done |
| **H12** | A self-contained linker: `tondo` links ELF and Mach-O executables itself, against the libraries the system has to run programs, with no C toolchain or development packages; position-independent executables, with `DT_RELR` where glibc reads it; `--static` for static executables with musl on Linux; `--strip`; unwind information for C++ exceptions; ad-hoc signatures on macOS | Done |
| **H13** | Diagnostics of running programs, all off unless a program starts with `TONDO_DIAGNOSTICS=1`: `tondo ps`; `tondo counters`, the runtime's counters and the program's own (`std.diagnostics`); `tondo trace`, a timeline of the tasks for Perfetto and a CPU profile; `tondo dump` and its reader, every task with its stack and locks and the live memory by site and type, also at a crash with `TONDO_DUMP`; and for `--debug` builds, `tondo debug` and `tondo dap`, with breakpoints, steps, bindings, and every object with what keeps it alive | Done |
| **H14** | Libraries for C: a `pub extern fn` with a body is a function C calls by its symbol; `tondo build --library` writes a shared library that publishes only those, which the toolchain's own linker makes as ELF and Mach-O, a static library, and a header of what they publish with its documentation; the runtime starts at the first call, and each call runs on the thread that makes it | Done |
| **Targets 2** | `--target x86_64-linux` and `aarch64-linux` from any machine: static executables, with another architecture's runtime in a pack that `tondo target pack` writes; `tondo run` and `tondo test` run them under qemu; CI builds `tondo` across and tests it under emulation | Done |
| **Live builds** | `--live`: a debug build whose code changes while it runs; `tondo reload` puts in what its files say now, keeping the program's state, and `tondo run --watch` does it as they are saved; under `tondo debug --live`, `reload` where the program stopped, Edit and Continue, and `restart` of the stopped frame; closures made before a reload run the new code, running frames move to the new code at their next statement with their values (on-stack replacement), and the values of a type that gains or loses fields or variants convert as their frames move, parameters, captures and numbers included, renamed fields, variants and types keeping their values, results and lent values converting back, and closures follow when their function gains or loses others; what `Ref`s, channels and `std.sync` values hold converts in place while every task stops at a statement, a task waiting for a channel, a lock or a line starting its statement again; through calls by slots, patches the toolchain's own linker lays out, and tables of the running image; in `tondo mcp`, sessions of live programs and `reload` | Done |
| **Agents** | `tondo setup`, which gives Claude Code and Codex the skill and the server; `tondo mcp`, a Model Context Protocol server in both eras of the protocol: every command as a tool with structured results, the compiler's fixes applied, the language server's navigation and rename, runs with input and a timeout, tests with their report, running programs observed and debugging sessions; the guide, the skill and the standard modules' APIs as resources | Done |

The first cut deliberately leaves out the following. They are part of the
language and remain planned work, scheduled after H7:

| Milestone | Scope |
|---|---|
| **Protobuf and gRPC** | Types and codecs generated from `.proto` schemas, and gRPC clients and servers over HTTP/2, designed together |
| **Windows** | Its calling convention, exceptions, network polling and executables |

What the specifications define and Tondo does not do yet, found by
compiling every example they contain and reading the rest against the
code (2026-10-24):

- **Tools:** the register `testing/doc-test-runtime-links.json`, which ties
  each fragment and script of a normative document to a test that runs it
  (21.6); `tondo doc-test` gives no `formatted_sha256` for an example the
  formatter cannot print on its own, a signature, a type or a pattern.

## Working rules

- Keep it small: a feature lands with its tests and nothing else.
- Specification changes are deliberate and reviewed; the implementation does
  not redefine the language.
- Tests are ordinary `cargo test` plus Tondo programs with stated
  expectations. There is no separate evidence ledger.

The previous implementation (bytecode VM first) is archived at
[tonyredondo/tondo-legacy](https://github.com/tonyredondo/tondo-legacy). It
remains useful as a reference and as an oracle for differential testing.
