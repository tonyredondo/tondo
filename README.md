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
agents and the editors it finds: the Tondo skill for Claude Code and
Codex, and `tondo mcp`, an MCP server whose tools do everything `tondo`
does, registered with both; and the Tondo extension, in
[`editors/vscode`](editors/vscode), installed in VS Code, Cursor, VSCodium
and Windsurf. Then `tondo guide` teaches the language, and in the editor
*Tondo: Get Started* walks through a first project.

With an agent, open this repository in it and ask it to set Tondo up:
[`AGENTS.md`](AGENTS.md) tells it how, and how to check that it works.
Other editors use `tondo lsp`; `tondo guide projects` says how to set one
up. Every CI run leaves the extension's `.vsix` as an artifact.

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
target/release/tondo graph                       # packages, modules, files and their imports
target/release/tondo new app                     # a project; `tondo add`, `remove`, `fix`, `explain E1102`
target/release/tondo script.to args              # for #!/usr/bin/env tondo
```

`tondo help` lists the commands and `tondo <command> --help` explains one;
`tondo completions bash|zsh|fish` completes them in a shell. With `--json`,
every command writes its result as one JSON object for tools.

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
| `editors/vscode` | The VS Code extension, in TypeScript: `npm test` runs its tests, `npm run package` makes the `.vsix` |

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
| **Stdlib 1** | The prelude: the everyday methods of `String` (`split`, `lines`, `repeat`, padding, trimming, Unicode case, parsing numbers), `Char`, `Int`, `Float`, `Option`, `Result`, `Array` (`map`, `filter`, `reduce`, `sorted`, `join`, `groupBy`, ...), `Map`, `Set` and lazy `Iterator` adapters; `Ordered`; `std.math` | Done |
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
| **H13** | Diagnostics of running programs, all off unless a program starts with `TONDO_DIAGNOSTICS=1`: `tondo ps`; `tondo counters`, the runtime's counters and the program's own (`std.diagnostics`); `tondo trace`, a timeline of the tasks for Perfetto and a CPU profile; `tondo dump` and its reader, every task with its stack and locks and the live memory by site and type, also at a crash with `TONDO_DUMP`; and for `--debug` builds, `tondo debug` and `tondo dap`, with breakpoints, conditions, counts of hits and logpoints, steps, the bindings of every frame and task opened part by part, values changed, a stop where a panic happens, every object with what keeps it alive, and dumps opened as a debugging session | Done |
| **H14** | Libraries for C: a `pub extern fn` with a body is a function C calls by its symbol; `tondo build --library` writes a shared library that publishes only those, which the toolchain's own linker makes as ELF and Mach-O, a static library, and a header of what they publish with its documentation; the runtime starts at the first call, and each call runs on the thread that makes it | Done |
| **Targets 2** | `--target x86_64-linux` and `aarch64-linux` from any machine: static executables, with another architecture's runtime in a pack that `tondo target pack` writes; `tondo run` and `tondo test` run them under qemu; CI builds `tondo` across and tests it under emulation | Done |
| **Live builds** | `--live`: a debug build whose code changes while it runs; `tondo reload` puts in what its files say now, keeping the program's state, and `tondo run --watch` does it as they are saved; under `tondo debug --live`, `reload` where the program stopped, Edit and Continue, and `restart` of the stopped frame; closures made before a reload run the new code, running frames move to the new code at their next statement with their values (on-stack replacement), and the values of a type that gains or loses fields or variants convert as their frames move, parameters, captures and numbers included, renamed fields, variants and types keeping their values, results and lent values converting back, and closures follow when their function gains or loses others; what `Ref`s, channels and `std.sync` values hold converts in place while every task stops at a statement, a task waiting for a channel, a lock or a line starting its statement again; through calls by slots, patches the toolchain's own linker lays out, and tables of the running image; in `tondo mcp`, sessions of live programs and `reload` | Done |
| **VS Code** | An extension over `tondo lsp`, `tondo dap` and `tondo mcp`: editing with every feature of the language server, formatting on save, *Run* and *Debug* over `main` or with F5, live builds that take in what is saved (Edit and Continue), the tests in the Testing view with failures on their lines, comparisons as diffs and stacks, continuous runs, coverage, and one test debugged alone, snapshots updated from a failure, the examples of Markdown documents, the modules of each project, running programs picked by name, their counters as they change, traces opened in Perfetto and dumps, tasks that read diagnostics as data into Problems, the arguments and environment of programs and tests in settings, a status bar with the version of `tondo` and a menu, *New Project* and a walkthrough to get started, and `tondo mcp`'s tools for VS Code's agents; its `.vsix` from every CI run | Done |
| **Tools** | The command line for tools: `--help` for each command and completions for bash, zsh and fish from one table of the commands; `--json` on every command, one object with its format and whether it did what was asked, and errors as JSON too; `--diagnostic-format json`; `tondo graph`, a program's packages, modules, files and imports as text, JSON, Graphviz or Mermaid, narrowed to what a module reaches or what reaches it, and the project and module of any path; `tondo test --test-format events`, a line for each start, output and result as it comes; `--list` from the tests' analysis alone, `--exact` again for more in one run, the sides of a failed comparison as data, and the features a client can rely on in `tondo version --json`; `tondo test --watch`, which runs again what a change can affect, through the imports; `tondo test --coverage`, lines, functions and the ways of branches the tests ran, as JSON and LCOV too; the language server sends each project's tests as they are written and offers *Run* and *Debug* over `main`, implementations, call and type hierarchies, the type of a binding, folding, selection and formatting of a range, the parameter of each argument, imports added as names are completed or fixed, a folder renamed with its imports, the problems of the workspace from the start, and each code linked to `tondo explain`; `tondo new`, `init`, `add`, `remove`, `fix`, `explain` and `clean`, also as tools of `tondo mcp`; `tondo mcp` and the VS Code extension use them: results live in the Testing view, the project of a file from `tondo graph`, and a check that `tondo` is new enough | Done |
| **Agents** | `tondo setup`, which gives Claude Code and Codex the skill and the server, and installs the extension in VS Code, Cursor, VSCodium and Windsurf; `tondo mcp`, a Model Context Protocol server in both eras of the protocol: every command as a tool with structured results, the compiler's fixes applied, the language server's navigation and rename, runs with input and a timeout, tests with their report, running programs observed and debugging sessions; the guide, the skill and the standard modules' APIs as resources | Done |
| **Benchmarks** | A test that calls `testing.bench(name, closure)` in its own body is a benchmark, known from the analysis; `tondo test` runs each closure once, and `tondo test --bench` measures the benchmarks alone and optimized: warm-up, 50 samples in `--bench-time`, and the allocations and bytes of a run, with the median, spread and every sample in the report and the machine they ran on; what a closure gives, and `testing.keep(value)`, stays out of the optimizer's reach; baselines saved with `--save-baseline` and compared with `--baseline`, or `--against` a git revision built and run alongside on the same machine, each change with its 95% interval and verdict; `--max-regression`, a gate that fails the run when a benchmark is slower or allocates more; and for any run, reports for people in Markdown and in one HTML file with charts; the language server marks benchmarks, VS Code measures them with its *Benchmark* profile, and `tondo mcp`'s `test` tool measures and compares them | Done |
| **Profiling** | `tondo profile`: where a running program, or one it starts, spends its processor time, what it allocates and holds, or where its tasks wait, in pprof's format with the function and line of every frame, the innermost too, from a table of lines the compiler keeps at no cost to run; `tondo profile show`, the functions and the program's own lines that take the most, as text, JSON or a flame graph in one HTML file; `tondo test --profile cpu|memory`, a profile of each test, or of what each benchmark measured; `std.profile`, profiles a program makes of itself, with labels per task its spawned tasks inherit, which every profile carries; in VS Code, *Profile* over `main` and in the Testing view, each hot line's share at its end and the flame graph beside the code; a `profile` tool for agents, the hot functions and lines with their code | Done |
| **Fuzzing** | A test that calls `testing.fuzz(name, seeds, closure)` in its own body tries inputs of any type with `Encode` and `Decode`, known from the analysis; `tondo test` runs each seed and each input that failed before, kept as JSON in the `fuzz/` store beside `snapshots/` and committed, so every one stays a test of regression; `tondo test --fuzz` searches until Ctrl-C or `--fuzz-time`, guided by the counters of the project's and its dependencies' code, with mutations that know the input's type and take values from the code's literals and comparisons, across `--jobs` workers sharing a corpus in `target/fuzz` or `--fuzz-corpus`; an input that fails, runs past `--fuzz-timeout`, holds more than `--fuzz-memory` or crashes is reduced while it fails the same way and kept, with the commands that run it again and `tondo debug --test`; VS Code marks each kind of test and searches with *Fuzz Test*, and `tondo mcp`'s `test` takes `fuzz` | Done |
| **Recover** | `recover { ... }`, a `T ! Panic` that stops a panic of its block, of what it calls and of the tasks it starts, with the panic's code, message, data, place and stack, and `p.resume()` to let it go on; panics with the program's own code and data, and `TONDO_PANIC_FORMAT=json`; the frames a panic leaves release what they hold and run their `defer`s by returning, each to its caller's handler, so that a call costs nothing more where nothing panics; `std.sync` locks let go, the block's other tasks cancelled, and `std.http` answering `500` to a handler that panics | Done |
| **Numbers** | Every width signed and unsigned, `Int8` to `Int128` and `UInt8` to `UInt128`, with `UInt` the unsigned `Int`, and both floats, with the same named operations as intrinsics: arithmetic that wraps, checks, saturates or tells the overflow, bits, bytes in either order, text in any base, conversions that keep the low bits or the nearest value, and the floats' rounding, roots and bits; `Additive` and `Number`, which give a program's types `+`, `-`, `*`, `/` and literals, and `Integer`, which gives a `T: Integer` all an integer has, with traits that require others; durations that add with operators; `Encode` and `Decode` of every number, integers past `Int` as their digits in JSON, YAML and CBOR; `Random.int` of any integer | Done |
| **Contexts** | `context Name = { field: T = initial }`, values every function of a task reads as `Name.field` without passing them, and `with Name { field: value }`, which changes them until the end of its block, unwinding too; a task takes the contexts of where it is spawned; `within limit { ... }`, a deadline for a block, every wait in it and the tasks it starts but detached ones, a `T ! time.TimedOut`, with `time.deadline()` and `time.remaining()`; `@log` and `@label` fields, in every log line, panic and profile; `log.Context`, the logger a task uses; the language server says which `with` gave a field its value, the debugger and VS Code show each task's contexts with the `with` that set each field, and so do dumps | Done |
| **Annotations** | `annotation Name = { field: T = default }`, data a program declares and writes as `@Name(...)` or `@module.Name(...)` on types, fields, variants, functions, methods and parameters, with constant arguments by position or name; read with their types through `std.reflect` (`field.annotation[Check]()`) and by `derive` providers and generators through `std.meta`, whose roots can now be public functions with their parameters and result; the language server explains, goes to and completes them, and their fields | Done |
| **Embedding** | `std.embed`: `embed.text`, `embed.bytes` and `embed.files`, files of the package read when the program compiles, at a literal path relative to the file, and kept in the executable's data without copying; `embed.Files` with `read`, `text`, `paths` and `list`, and `app.embedded`, which serves them from memory with ranges, `ETag`s and a folder's `index.html`; embedded files are inputs of the build, so they rebuild `tondo run` and `tondo test`, reload `--watch` and show in `tondo graph`; the language server goes to them and completes their paths | Done |
| **Systems** | What a tracer needs of the system: MD5 and SHA-384, digests and HMACs in parts, and FNV, xxHash64 and MurmurHash3 with the values of their reference implementations; Unix sockets, streams and datagrams, with HTTP clients and servers over them; the hostname, the process id and the executable; `SIGINT` and `SIGTERM` that cancel the program so its `defer`s run, `process.signals` for those a program handles itself and `Process.signal` for its children; `request.route()`, the template of the route that answers, which middleware sees; a MessagePack writer in parts | Done |
| **Web** | What a web framework needs: `std.url`, URLs and relative references as RFC 3986 reads and resolves them, with the query of a form; cookies on both sides, read and set by a server and kept by a client as a browser does; `net.Ip` and `net.Network`, addresses in their canonical form and networks in CIDR notation, and `request.clientIp()` behind the proxies an app trusts; HTML escaping, with every named reference of HTML5; regular expressions with the `i`, `m`, `s` and `U` flags in the pattern, replacements by a function and `Regex.escape` | Done |
| **Sites** | What a site generator needs: `path.clean`, `relative` and `matches`, glob patterns with `**` and braces; `fs.walk`, `fs.glob`, temporary directories and files, a file's modification time and permissions, symbolic links, and `fs.watch`, changes in batches once they settle; `writeText` that keeps a file's permissions and writes through a link; `std.gzip` and `std.zstd`, whole or as streams; time zones of the tz database, the system's or the copy in `tondo`, a `ZonedDateTime` with the times clocks skip or show twice, `strftime` patterns that format and parse with the names of months and days of CLDR's languages and are checked when literal, and `time.parseDuration` | Done |
| **Streams** | `io.pipe`, a writer and a reader across tasks with a capacity, for a reply written while it is sent; `io.copy`, `io.multiWriter` and `Reader.readBytes`; a request's `host`, `scheme`, `rawQuery` and `url`, from a trusted proxy's `X-Forwarded-Host` and `X-Forwarded-Proto` too; hooks around a client's requests, as middleware is around a server's routes; and channels whose producer sees that no receiver is left, and can drop what it sends then | Done |
| **Introspection** | `diagnostics.metrics()`, the counters `tondo counters` shows, which a program that reads them counts from its start; `diagnostics.stack()`, the frames of the task that asks; profiles of where tasks wait for others and of where locks others waited for are let go, sampled by time waited, as Go's block and mutex profiles; and profile labels set and restored without a block, for a span | Done |
| **Text** | Formats in interpolation, `"{price:>10.2}"`, `{id:08}` and `{mask:#x}`, as Python and Rust write them, checked against the value's type when the program compiles; `text.normalize` and `isNormalized`, the four forms of Unicode; `std.xml`, XML written from a program's types and read into them through the serialization every format shares, with attributes, text and names by annotation and scalars read from their text, and a tree of XML of unknown shape, with no DTD or entity but the predefined ones | Done |
| **Templates** | `std.template`: text and HTML templates whose actions are Tondo's expressions, `{{ for post in posts }}` and `{{ price:.2 }}`, with layouts that `extends` and `block` make and `include`; compiled with the program from a raw string or an embedded file, checked against their data's type and calling the functions their file sees, or loaded while it runs into a `Set` that sees the data by reflection; HTML escaped for where each value goes, as Go's `html/template` does, and lines that hold only blocks left out | Done |
| **Command lines** | `std.cli`: the command line read into the program's types, a record of options and arguments or an enum of subcommands, with the help their `///` give, typos answered with the nearest name, and completion for bash, zsh and fish; Spectre's markup, `[bold red]text[/]`, whose interpolated values stay text and whose styles are checked when the program compiles; tables of records, progress bars and spinners, and questions a script can answer. `std.mcp`: Model Context Protocol servers from the `pub` methods of a value, their arguments and schemas from the types and their documentation, over standard input and output or a route of `std.http`. Reflection gives the documentation of types, variants and methods | Done |
| **Everyday** | `String.splitOnce`, `split` with a limit, `equalsIgnoringCase` by Unicode's case rules, and `trim`, `trimStart` and `trimEnd` of given characters; `env.homeDirectory`, `configDirectory` and `cacheDirectory`, `env.processors()` within the program's affinity and cgroup, and `env.build()`, the program's name and `version` from `tondo.toml`, its git commit, target, compiler and dependencies; `format.apply`, a format chosen while the program runs; and numbers, percents, amounts of money and plural forms as each language writes them, from CLDR | Done |
| **Security** | `std.crypto`: secrets compared in constant time, and `==` on bytes and digests too; passwords hashed with Argon2id and those of bcrypt, PBKDF2 and Argon2 read; HKDF and PBKDF2; data sealed with AES-256-GCM or ChaCha20-Poly1305, its nonce made for it; Ed25519, ECDSA and RSA signatures; the keys OpenSSL writes and X.509 certificates read from PEM. TLS from PEM files or text, mutual TLS with the client's certificate in the request, a lowest version, and `startTls` for protocols that start plain. Record literals of other modules' types, `time.DateTime { ... }` | Done |
| **Servers** | `std.validate`: rules on the fields of a type, `@validate.Length`, `Min`, `Max`, `Matches` and `Email`, checked by the `Decode` the compiler writes so that no value read from outside breaks them, answered with `422` and each violation by `request.json`, checked on the command line and on the arguments of MCP tools, and checked against their fields when the program compiles; `App.client()`, the app answering in the same process for tests; `http.proxy`, a reverse proxy whose requests the client's hooks observe; `http.mediaType`; `sync.RateLimiter` and `sync.retry` with jittered backoff | Done |
| **Exact numbers** | `math.BigInt`, an integer of any size, and `math.Decimal`, an exact decimal with rounding modes, written in Tondo with Knuth's long division; literals that are exact where one is expected, of any size; formats of interpolation for them, `"{price:.2}"`; and both in every format of serialization | Done |
| **Trees and archives** | `fs.FileSystem`, a tree of files with the operations of `std.fs`: a directory of the disk, files in memory, the files `embed.files` keeps in the executable, an archive's, the part under a directory of another, or several in layers; `App.files` serves one and `template.Set.addFiles` loads one. `std.archive`: zip, tar, tar.gz and tar.zst read, written and extracted in Tondo, with ZIP64, PAX and GNU long names, CRC-32 checked, and entries that would leave the directory refused | Done |
| **Databases** | `std.sql`: SQLite in the runtime, and PostgreSQL and MySQL written in Tondo, through one API whose queries are string literals with their values as parameters, so they cannot be injected; rows read into records, values, options, JSON columns and arrays; a pool of connections that tasks share, transactions with savepoints that roll back on failure and panic, migrations from a tree of files, and `use` around each statement for traces and metrics; TLS always verified, SCRAM, MD5 and `caching_sha2_password` | Done |
| **Images** | `std.image`: PNG, JPEG, GIF and WebP read and written, with the EXIF orientation applied; resize keeping proportions, fit, fill with an anchor, crop, rotate, flip, grayscale, blur, overlay and pixels; linked only by programs that import it | Done |
| **Automatic certificates** | `std.acme`: `acme.tls(["example.com"], email: ...)` gives a server Let's Encrypt's certificates, obtained and renewed while it runs, checked with TLS-ALPN-01 on its own port and kept in a cache; ACME, JWS and the DER of requests and certificates in Tondo; `net.Certificates`, certificates that change while a server shows them | Done |
| **GraphQL** | `std.graphql`: a GraphQL server made of the program's types, the fields and `pub` methods of a query's and a mutation's roots and of each type they give, with docs as descriptions; queries parsed and run in Tondo over reflection, with fragments, directives, variables, errors with path and null propagation, and introspection; SDL, a route of `std.http` and GraphiQL; roots that hold a `sql.Database` or `std.sync` state | Done |
| **Servers in production** | Limits on each client by default: the size of a body (`413`, raised per route with `request.limitBody`), the size of a head (`431`) and the time to send it or finish TLS; a client that goes away cancels the handler that answers it; `request.queryAs[T]()` and `formAs[T]()`, the query and forms read into types; `headerValues` and `Reply.headers`; `App.compress`, zstd or gzip, streams too; WebSocket handlers with their request, inside the middleware's contexts | Done |
| **Shared values** | Context fields whose initial values are computed once, when the program first reads them, and that every task then shares: compiled patterns, pools and `std.sync` state, with no globals and nothing to set up in `main`; a failing `?` panics at the read, which the next one retries; a `with` leaves the other fields uncomputed; cycles among them are build errors; literal patterns of `Regex.compile` are checked when the program compiles | Done |
| **Processes and the system** | `Command.group()`, a process group of its own that `cancel` and `signal` reach whole, with what it started; `process.signal(pid, ...)` to any process, `Quit` and `Kill`; `process.which`; hard links, `fs.setModified`, `fs.space`, file locks that wait without a thread, and the inode, links, owner and group of an entry; `env.user()`, `env.system()` and the machine's cores, read once | Done |
| **Config files and tools** | `yaml.Document`, a YAML file edited in place: `find` gives a value with its line, column and comment, `set` changes or adds one and leaves the rest of the text byte for byte, `writeTo` writes it without a copy; YAML merge keys `<<`; `@omitNone` on a field or a type, in every format; `json.encode(htmlSafe: true)`; `std.cli` options that also work after a subcommand, `@cli.Rest` for what a command passes on, `time.Duration` options and options read from the first of several variables | Done |
| **Templates for sites** | Functions of a `template.Set` called as Tondo calls them, with defaults, variadic arguments and `T ! E`; `include` with values by name, typed when compiled, and by a computed name when loaded; literals of arrays, maps and characters, and strings with values in them; JSON, YAML and TOML values read as what they hold, and URLs by their parts | Done |
| **Calendar** | `addDays`, `addMonths` and `addYears` that keep the clock's time, at the end of a shorter month and across a change of the clocks; `add` and `since` with durations; `weekday`, `dayOfYear` and the week of ISO 8601, written and read with `%u %w %U %W %V %G %g`; dates in loaded templates with their fields and methods | Done |
| **Unicode text** | `text.category` and `text.script` of any character, as enums, and `isPunctuation`, `isSymbol` and `isMark`; `text.collator(locale)`, the order of each language of CLDR, at a strength and with numbers by their value, whose data only the programs that use it carry | Done |
| **Image colors** | `adjust` of brightness, contrast, saturation, gamma and hue in the scales of Hugo's filters; `invert`, `sepia`, `sharpen`, `pixelate` and `opacity`; `colors(n)`, the colors that cover most of an image; its pixels as bytes, and an image made of them | Done |
| **Profiles per period** | `cpu.period()`, the processor time since the last period as sampling goes on; `later.since(earlier)`, what allocations and waits added between two profiles, by stack and labels, with the period's start and length in the pprof; how often each profile samples, `every` | Done |
| **Files per system** | `watch_linux.to`, `watch_macos.to` and `signals_unix.to`, which a build takes for its system, with no `#if`; `tondo check` and the language server check every system and name the one an error is of, say where a name missing for one is and what to write, and require one signature for every system; `systems` in `[package]`; in the editor, a badge on each such file, the other system's files read as that system reads them, definitions of each system, the system in the status bar, breakpoints and tests of another system's files shown and not run, and coverage that leaves them out | Done |

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
