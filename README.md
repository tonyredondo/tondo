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
    _ = console.print("Hello, world\n")
}
$ tondo run hello.to
Hello, world
```

## Using it

Building requires Rust (the version is pinned in `rust-toolchain.toml`) and a C
toolchain (`cc`), which is used as the linker.

```text
cargo build --release
target/release/tondo run program.to -- args      # compile (cached) and run
target/release/tondo build program.to --output app
target/release/tondo check program.to
target/release/tondo script.to args              # for #!/usr/bin/env tondo
```

`tondo run` works like `go run`: it compiles the program to a native
executable, caches it under `~/.cache/tondo`, and executes it. There is no
interpreter, so `run` and `build` always agree.

## How it is built

| Path | Contents |
|---|---|
| `crates/tondo` | Compiler and CLI: lexer, parser, checker (AST to typed HIR), MIR lowering with explicit reference counting, Cranelift code generation, linking |
| `crates/tondo-runtime` | Runtime linked into every executable (strings, console, panics). The compiler's build script compiles it as a static library and embeds it |
| `tests/cases` | Tondo programs whose comments state the expected output, exit status, panic or diagnostic |
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
| **H1** | Data: tuples, records, newtypes, aliases, enums (including recursive ones), `match` with exhaustiveness, patterns, `Option`/`Result` with `ok`/`err`/`some`/`none`, `?` and `fail`, fallible `main`, `Char`, `Byte`, floats, numeric conversions, `Array`/`Map`/`Set` with copy-on-write | Mostly done; remaining: structural unions (`A \| B`, error unions), array arithmetic, slice assignment, string indexing and slicing |
| **H2** | Abstraction: methods, generics (monomorphization), traits with static dispatch, closures, iterators, variadics | Planned |
| **H3** | Ownership: affine values, `ref`/`mut`/`var` loans, `defer`, terminal resources, panic unwinding | Planned |
| **H4** | Projects: modules and packages, `tondo.toml` and lockfile, canonical formatter (`tondo fmt`), JSON diagnostics | Planned |
| **H5** | Concurrency: stackful tasks, `spawn`/`scope`/`Join`/`await`, cancellation, `select`, channels, `spawn thread` | Planned |
| **H6** | Hosted standard library: filesystem, processes and pipelines, environment, time, JSON | Planned |

The first cut deliberately leaves out the following. They are part of the
language and remain planned work, scheduled after H6:

| Milestone | Scope |
|---|---|
| **H7** | `unsafe` regions and `Pointer[T]` |
| **H8** | The complete testing framework of `TONDO_TESTING_SPEC.md` (suites, isolation, virtual time, snapshots, retries, reports) |
| **H9** | Static metaprogramming (`derive`, generators) and runtime reflection |
| **H10** | Tondo LLM Form (`TONDO_LLM_FORM_SPEC.md`) |
| **H11** | Race, leak and crash diagnostics; language server; incremental compilation |
| **H12** | Further targets (Linux ARM64, macOS, Windows) and a self-contained linker |

## Working rules

- Keep it small: a feature lands with its tests and nothing else.
- Specification changes are deliberate and reviewed; the implementation does
  not redefine the language.
- Tests are ordinary `cargo test` plus Tondo programs with stated
  expectations. There is no separate evidence ledger.

The previous implementation (bytecode VM first) is archived at
[tonyredondo/tondo-legacy](https://github.com/tonyredondo/tondo-legacy). It
remains useful as a reference and as an oracle for differential testing.
