<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/branding/tondo-wordmark-dark.svg">
    <img src="docs/branding/tondo-wordmark.svg" alt="Tondo" width="332">
  </picture>
</p>

Tondo is a statically typed language that compiles to native executables. It is
meant to be light to write, with little to annotate and errors that say what to
type, and complete when a program grows: one binary, `tondo`, is the compiler, the
standard library, the test runner, the formatter, the debugger, the language
server and an MCP server for coding agents.

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

`tondo run server.to` starts it. The route's `id` arrives as an `Int`, the record
is written as JSON, and `none` is a 404.

Tondo is in early development and nothing is released yet. It runs on Linux and
macOS, on x86-64 and arm64; [Status](#status) says what is done and what is not.

## What it is like to write

- **Errors are in the signature.** `T ! E` says a function can fail, `?` passes the
  failure on, and a result nobody looks at does not compile.
- **Any function can wait.** Tasks have no colored functions: `spawn` starts one,
  and a call that waits is written like any other.
- **Values behave like values.** Assigning copies, as far as anyone can tell, and
  memory is managed for you, cycles included.
- **Mistakes say what to do.** `name.upper()` gives
  `` `String` has no method `upper`: use `toUpper()` ``, and `tondo fix` applies
  the repairs the compiler is sure of.
- **There is one way to write it.** `tondo fmt` has no style to choose, and a
  project needs little more than a `tondo.toml` with its name.

## What comes with it

| Area | What you get |
| --- | --- |
| Standard library | HTTP/1.1 and HTTP/2 clients and servers, WebSockets, gRPC and Protobuf, GraphQL, SQL for SQLite, PostgreSQL and MySQL, JSON, YAML, TOML, XML and CBOR, templates, images, command lines, automatic certificates |
| Testing | `tondo test` with coverage, benchmarks compared against a commit, fuzzing, and a search over the orders of a test's tasks that finds lost updates |
| Debugging | Breakpoints that stop one task of a running Release build while the rest go on, stepping back, record and replay, and the question of why a value is what it is |
| Failures | Dumps of a crash or of a running program with what each task held, grouped by failure, and made into a test that repeats it |
| Live editing | A running program takes in what you save and keeps its state; a panic waits for your fix |
| Editors | A VS Code extension with all of this, and `tondo lsp` for any other editor |
| Coding agents | `tondo mcp` gives every command as a tool, with runbooks for what to do when something goes wrong |
| Builds | Static executables, builds for another architecture, libraries for C, and its own linker: no C toolchain to run a program |

`tondo guide` teaches the language a topic at a time, for people and for models.

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
`tondo-<target>` (x86-64 and arm64 Linux with glibc 2.34 or later and musl, which runs on older systems too, macOS on Apple
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

## Status

Nothing is released yet, and the language can still change. What each milestone
covers, with what it took, is in [`docs/roadmap.md`](docs/roadmap.md).

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

Known limits today: logpoints and Release breakpoints work on Linux only, since
macOS does not let a program change its code.

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

## Working rules

- Keep it small: a feature lands with its tests and nothing else.
- Specification changes are deliberate and reviewed; the implementation does
  not redefine the language.
- Tests are ordinary `cargo test` plus Tondo programs with stated
  expectations. There is no separate evidence ledger.

The previous implementation (bytecode VM first) is archived at
[tonyredondo/tondo-legacy](https://github.com/tonyredondo/tondo-legacy). It
remains useful as a reference and as an oracle for differential testing.
