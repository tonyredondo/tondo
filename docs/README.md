# Tondo's documentation

Where everything about Tondo is written, and which to read for what.

## Start here

| Read | For |
| --- | --- |
| [A tour of the language](tour.md) | The language by example, from a script to tasks and C |
| [The toolchain](toolchain.md) | What `tondo` does besides compiling, with its commands and what they print |
| [Tondo next to other languages](comparison.md) | What it shares with Go, Rust and C# and where it differs, and a page of programs for each of eight languages |
| [The README](../README.md) | What Tondo is, how to install it and where it stands |

## The guide

`tondo guide` is the reference, a topic at a time, written for people and for
models. `tondo guide <topic>` prints one in the terminal; the same text is in
[`crates/tondo/guide`](../crates/tondo/guide), and the tests compile its
examples.

| Topic | What it covers |
| --- | --- |
| [overview](../crates/tondo/guide/overview.md) | The language on one page, the commands, and how an agent works with them |
| [syntax](../crates/tondo/guide/syntax.md) | Declarations, control flow, `match`, closures, loops |
| [types](../crates/tondo/guide/types.md) | Records, enums, newtypes, unions, generics, traits, numbers |
| [errors](../crates/tondo/guide/errors.md) | `T ! E`, `?`, `fail`, `Option`, `Failure`, panics, `recover` |
| [collections](../crates/tondo/guide/collections.md) | `Array`, `Map`, `Set`, `Deque`, ranges, iterators |
| [text](../crates/tondo/guide/text.md) | Strings, characters, bytes, formatting |
| [tasks](../crates/tondo/guide/tasks.md) | `spawn`, `Group`, channels, `select`, cancellation, `std.sync`, contexts, deadlines |
| [projects](../crates/tondo/guide/projects.md) | `tondo.toml`, modules, imports, dependencies, editors, agents |
| [testing](../crates/tondo/guide/testing.md) | `test`, `suite`, snapshots, benchmarks, fuzzing, every order of tasks |
| [models](../crates/tondo/guide/models.md) | `std.model`: every state of a design or of the code, and what must hold |
| [stdlib](../crates/tondo/guide/stdlib.md) | The standard modules and what each is for; watching, debugging, live builds |
| [http](../crates/tondo/guide/http.md) | HTTP client and server, typed handlers, OpenAPI, WebSockets, GraphQL, gRPC |
| [templates](../crates/tondo/guide/templates.md) | Text and HTML templates checked against their data |
| [databases](../crates/tondo/guide/databases.md) | SQLite, PostgreSQL, MySQL: queries, rows, transactions, migrations |
| [meta](../crates/tondo/guide/meta.md) | `derive`, reflection, generators, annotations |
| [ffi](../crates/tondo/guide/ffi.md) | Calling C, and libraries for C |
| [cli](../crates/tondo/guide/cli.md) | Every `tondo` command |
| [mcp](../crates/tondo/guide/mcp.md) | Every tool of `tondo mcp`, for agents |
| [pitfalls](../crates/tondo/guide/pitfalls.md) | Habits from other languages, with the error each gives and the fix |
| [runbooks](../crates/tondo/guide/runbooks.md) | What to do, step by step, when something goes wrong: a crash, a hang, a flaky test, a slow request |

`tondo api <module>` shows what a module of the standard library has, with
its documentation and examples, and `tondo explain <code>` what an error, a
warning or a panic code means.

## The specifications

The specifications in [`spec`](../spec) define Tondo, and the implementation
follows them. They are written in Spanish.

| Specification | Defines |
| --- | --- |
| [Language](../spec/TONDO_LANGUAGE_SPEC.md) | Syntax, types, ownership, errors, tasks, with the grammar and what was left out on purpose |
| [Standard library](../spec/TONDO_STANDARD_LIBRARY_SPEC.md) | Every standard module and its contract |
| [Testing](../spec/TONDO_TESTING_SPEC.md) | Tests, suites, reports, benchmarks, fuzzing, orders of tasks |
| [Toolchain](../spec/TONDO_TOOLCHAIN_SPEC.md) | The commands, projects and dependencies, builds, diagnostics |

## The project

| Read | For |
| --- | --- |
| [Roadmap](roadmap.md) | What each milestone covers, and what is left |
| [Design decisions](decisions.md) | Why things are as they are, a short record for each decision |
| [Benchmarks against Go](../bench/README.md) | The same work in Tondo and in Go, with the numbers and how to run them |
| [Speed plan](speed.md) | The plan for the compiler's and the runtime's speed, phase by phase |
| [Refactoring plan](refactor.md) | How the runtime and the standard library were split, and what holds them |
| [Tondo for agents](../AGENTS.md) | Setting Tondo up, writing it, and working on the compiler |
| [VS Code extension](../editors/vscode/README.md) | What the extension does in the editor |
| [Packages](../dist/README.md) | What a downloaded `tondo` contains and needs |
| [Cranelift, vendored](../vendor/README.md) | Tondo's copy of the code generator, and the rules it adds |
| [Logo](branding/README.md) | The mark, the wordmark and their colors |
