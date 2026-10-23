# Tondo

Tondo is a statically typed, compiled language with automatic memory
management: *small by design, complete in practice*. The normative definition
lives in [`spec/`](spec/); this repository is a fresh, native-first
implementation of it.

**Status:** early development, nothing is released. The first milestone (H0)
compiles a small subset of the language to native executables with
[Cranelift](https://cranelift.dev).

```text
$ cat hello.to
import std.console

fn main() {
    console.println("Hello, world")
}
$ tondo run hello.to
Hello, world
```

## Using it

Every CI run leaves a ready `tondo` for each platform as an artifact,
`tondo-<target>` (x86-64 and arm64 Linux with glibc and musl, macOS on Apple
silicon and Intel): `bin/tondo` is the whole toolchain, and it needs only a
C toolchain (`cc`) to link (`dist/README.md`).

Building requires Rust (the version is pinned in `rust-toolchain.toml`) and a C
toolchain (`cc`), which is used as the linker.

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

The first cut deliberately leaves out the following. They are part of the
language and remain planned work, scheduled after H7:

| Milestone | Scope |
|---|---|
| **H11** | Race, leak and crash diagnostics; language server; incremental compilation |
| **H12** | Windows, and a self-contained linker |
| **Protobuf and gRPC** | Types and codecs generated from `.proto` schemas, and gRPC clients and servers over HTTP/2, designed together |

## Working rules

- Keep it small: a feature lands with its tests and nothing else.
- Specification changes are deliberate and reviewed; the implementation does
  not redefine the language.
- Tests are ordinary `cargo test` plus Tondo programs with stated
  expectations. There is no separate evidence ledger.

The previous implementation (bytecode VM first) is archived at
[tonyredondo/tondo-legacy](https://github.com/tonyredondo/tondo-legacy). It
remains useful as a reference and as an oracle for differential testing.
