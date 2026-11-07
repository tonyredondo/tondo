# Refactoring the runtime and the standard library

The runtime and the standard library grew a feature at a time, and the cost now shows: two bugs on
one day came from state a task kept on a thread it no longer ran on, the scheduler's core knows
about every tool, and a few modules are thousands of lines each. This is the plan to restructure
them without changing what they do, in phases, each one pinned by tests before it moves anything.
Bugs the tests find on the way are fixed as they come, each in its own commit.

## What it starts from

Measured on 2026-11-07, at 9439931:

- The runtime: 53,000 lines of Rust in 69 files, 845 `tondo_rt_` functions that compiled code
  calls, 1,234 `unsafe` blocks, 346 statics, 73 of them a global `Mutex`, 49 places that read the
  environment, 98 conversions of a Tondo string with `from_utf8_lossy` though `str_text` reads one
  without a copy and is used 12 times.
- `tasks.rs`, 5,100 lines: stacks, the workers and their queues, stopping the world, calls of C,
  waits and locks, joins, groups and channels, using 27 other modules; `run()`, at every switch of
  task, checks 13 aspects of the tools. `debugger.rs` is 5,200 lines, `lib.rs` 3,000.
- The standard library: 45,000 lines of Tondo in 70 files; `http.to` 3,900 lines, `sql.to` 2,900
  with its three drivers, `grpc.to` and `protobuf.to` over 2,000.
- What guards them: 523 cases run in five modes on five platforms, about 300 integration tests,
  the 11 models of the runtime under TLC and `std.model`, the 25 tests of `tests/orders`, the
  benchmarks against Go. 26 unit tests in the runtime, no check that the symbols compiled code calls
  are the ones the runtime exports, no fuzzing of the runtime's parsers, and modules of the standard
  library with no case of their own.

## Rules for every phase

- Nothing moves before phase 0 has pinned it.
- A commit that only moves code goes alone, apart from one that changes anything.
- No change of behaviour or specification unless the phase says so; Release gets no slower.
- A phase ends published, with its gate passed: fmt, clippy and the workspace's tests; the cases
  in debug, replay, live and static builds; the containers of `.github/scripts/local.sh`, musl and
  one core; a round on macOS for arm64; the Nightly; the models with TLC and `std.model`; the
  baseline of performance and allocations of phase 0.7, within 5% and no more allocations, as
  `bench/compare.to --old` measures it (bench/baselines/pre-refactor.md).

## Phase 0: pin what it does

1. **Coverage of the runtime.** A build of the runtime with `-C instrument-coverage`, the cases
   and the integration tests run against it, the profiles merged with `cargo llvm-cov`: the
   functions and branches nothing runs, first those of the files the later phases move. The same
   for the Tondo code of the standard library, with an internal switch that counts it.
2. **Tests that pin behaviour** where coverage shows gaps: cases with their output for strings,
   arrays, maps and sets at their edges; channels, `select`, groups, joins, cancellation,
   `within`, timeouts and `defer` in tasks; the cycle collector, contexts, signals; panics in
   tasks and `recover`. A case pins what happens today, specified or not.
3. **The contract of the calls.** A test that reads the compiler's table of the runtime's
   functions, `Rt` and `NATIVES` with their signatures, and the symbols the runtime exports, and
   fails when one is missing or differs; it lists those nothing calls.
4. **Formats.** Golden tests, with what varies normalized, of the panic report as text and as
   `tondo-panic-0.1/1`, the dump, the JSON report of `tondo test`, the trace, a pprof profile and
   the human output of `tondo test`.
5. **The public API of the standard library.** A snapshot of `tondo api` of every module, held by a
   test, written again only on purpose.
6. **Fuzzing the parsers, in Tondo.** `testing.fuzz` tests of JSON, YAML, TOML, CBOR, MessagePack,
   XML, URLs, regular expressions, templates and protobuf, which go through the runtime: no input
   panics, and what is encoded reads back the same. The corpus stays in the repository; the
   Nightly searches.
7. **Baselines.** `bench/compare.to` against Go; benchmarks of the runtime with allocations per
   operation, spawn and join, a channel's round trip, a contended mutex, `select`, maps, the string
   builder, JSON and YAML; the size of an executable and its start. Saved as `pre-refactor`.
8. **Mutation testing** with `cargo mutants` of the scheduler's core and of strings and arrays,
   at night: a mutant that survives is a missing test, or is explained as one that changes
   nothing.
9. **Weak memory.** Every case that starts tasks run a hundred times on arm64 macOS, each with
   its time limit and its checks, part of the gate of phases 1 and 2:
   `TONDO_CASES_REPEAT=100 cargo test -p tondo --test cases cases` on the M2.

## Phase 1: one context for a worker

What a worker keeps, the running task, its own stack, its next task and queue, its calls of C,
stopping the world, its spare stacks and turns, in one structure reached through one function
that never inlines, asked for again after anything a task may switch threads at. A test fails on
a new `thread_local!` outside it in code that runs on a task's stack.

## Phase 2: the scheduler apart from the tools

`tasks.rs` split into switching and stacks, the workers and their queues, parking and waking,
timers, calls of C and the monitor, stopping the world, waits, joins, groups and channels. The
tools, the debugger, recording and replay, the race detector, the trace recorder, live builds and
profiling, attach through one interface, behind one flag the hot paths look at. The comments of the
models follow the functions they name.

## Phase 3: configuration read once

The variables of the environment the runtime reads, in one structure read at the start, the source
of their table in the specification.

## Phase 4: strings and values across the boundary

Views of a Tondo string or array from Rust that neither check nor copy, one way to give them back,
and one helper for an error code with a value out, as `std.fs`, `std.net`, `std.http` and
`std.sql` repeat it. Measured in allocations per operation.

## Phase 5: `lib.rs` split

Strings, arrays, maps and sets, the string builder, boxes, references, function values, the
console and panics, each a module.

## Phase 6: the tools split

`debugger.rs` into probes, stopping, bindings and commands; `diagnostics.rs` into dumps and the
server of the tools; `replay.rs` into recording and replaying.

## Phase 7: the large modules of the standard library

`http.to` into its client, server, router, cookies, WebSockets and OpenAPI; `sql.to` into what is
common and a module for each driver; `grpc.to` and `protobuf.to`; all internal modules, the public
API held by the snapshot of phase 0.5. Independent of the runtime's phases.

## Phase 8: performance and allocations

Profiles of the benchmarks and of allocations, and only changes that measure better: the global
lock of the scheduler's queue, allocations in JSON and serialization, the fingerprints of
`std.model`, the pairs of retain and release the optimizer leaves.

## Found on the way

Bugs the work of phase 0 found, each fixed in its own commit with its test: on macOS a dump lost
the bindings of the tasks that waited, and an outermost frame broke the copy that shows them; the
workers of `tondo test` ran one at a time; `run` of `tondo mcp` counted the build in a program's
time; a string built while the tools count memory was copied whole at each append; and the fuzz
tests of 0.6 found five bugs in `std.xml`, `std.yaml` and `std.url`, and writing them a misnamed
protobuf encoding that the compiler blamed on the field's type.

## Status

| Phase | State |
| --- | --- |
| 0 | started 2026-11-07: 0.3 to 0.7 done, the baseline in bench/baselines; 0.1 measured, 83% of the runtime's lines, without replay or the standard library's Tondo code; 0.2 under way; 0.8 and 0.9 to do |
| 1–8 | not started |
