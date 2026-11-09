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
`std.model`, the pairs of retain and release the optimizer leaves. Phase 4 measured where those
allocations are: decoding a record of four fields from MessagePack or CBOR takes 93 of them, from
JSON 12, nearly all an event of `std.serialization` with its own string for each key and value;
and the reader of JSON copies its whole input, where it could hold the string.

## Found on the way

Bugs the work of phase 0 found, each fixed in its own commit with its test: on macOS a dump lost
the bindings of the tasks that waited, and an outermost frame broke the copy that shows them; the
workers of `tondo test` ran one at a time; `run` of `tondo mcp` counted the build in a program's
time; a string built while the tools count memory was copied whole at each append; and the fuzz
tests of 0.6 found five bugs in `std.xml`, `std.yaml` and `std.url`, and writing them a misnamed
protobuf encoding that the compiler blamed on the field's type. Then the Nightly's: a walk of the
stack, for a sample of the heap, read address 0 at a frame pointer near the top of memory; a test
whose task spun, or whose `select` kept finding a closed channel, stayed in its first order until
its time ran out; and the script of the models took a broken invariant for an output it did not
know. The baseline found an empty array literal allocated at every call of a derived encoder.
Measuring the runtime's coverage under replay found that Tondo's linker could not link a C library
that gathers entries in a section and walks them from `__start_` to `__stop_` of its name, nor one
that calls `atexit`, which glibc keeps in `libc_nonshared.a`. Mutation testing found what no test
held: that a text built by appending doubles its room, which could turn quadratic unseen, and that
a seeded order starts every task above the priorities of PCT's changes. One of the cases written
for them found a lost wake in the scheduler: a worker woken for a task, which found nothing, counted
the next wake as none, and a task woken next to one that then computed without waiting could wait
until it stopped, once in twenty runs on Linux and in a hundred on the M2. Closing the mutants it
left open found two more in how a program ends after a panic: the end of `main`, which cancels the
detached tasks once more, cut short the wait of a cleanup that a `within` gives until its deadline;
and when `main` itself panicked with one worker, as on one core, exploring orders or recording, the
other tasks' cleanups never ran, and the program waited out its five seconds. A case written for
another found that a recorded run whose callback of C waited for a process never ended: the thread
that stood in for its worker waited in the runtime it drives, which nothing told that the task went
on.

## Status

| Phase | State |
| --- | --- |
| 0 | done 2026-11-08: 0.3 to 0.7 and 0.9 on 2026-11-07, the baseline in bench/baselines. 0.1: `.github/scripts/coverage.sh` measures what the tests run of the runtime, 88.4% of its lines, the cases under replay included now that Tondo's linker links the runtime that counts, and with `TONDO_COVERAGE_STD` what the cases run of the standard library's Tondo code, 74.8% of the lines they build; PostgreSQL's and MySQL's drivers and OpenAPI run only in the tests with servers, which it does not count. 0.2: cases where it showed gaps, among them a context's value that other tasks wait for, an actor's mailbox, `group.next()` in a `select`, semaphores, barriers and readers that starve, arithmetic on arrays of every type of number, a task's panic that reaches a waiter's cleanup, a cycle through a `reflect.Value` and a `within` that never waits. 0.8: all 168 mutants once; the cases caught 64, 8 did not build, and of the 96 left 19 are caught by tests of the toolchain the script did not run, which it runs now, 25 by new tests, 41 change nothing anyone can see, as 38 lines of testing/mutants/explained.txt say, and one went with the line the fix of a lost wake changed. Of the ten left open then, eight have tests since: of `wake_idle` in the runtime's own tests, a case of a deadlock after the workers started in place of those blocked in C retire, a test of the thread that stands in, exploring orders, for a worker that a callback of C holds while it waits for a process, and a case of a panic of `main`'s own. One, a retiring worker that left tasks without waking another, is gone: a worker that retires now always wakes one, since the case of a deadlock after the workers that replaced calls of C retire hung once in a hundred runs, where the last to sleep had counted the retiring one as busy and nobody looked again, which the model of waking, without deadlocks, does not see (docs/speed.md, C). One stays open: a worker that looks for tasks and finds one elsewhere wakes the next if the shared queue filled meanwhile, which neither a test nor the model shows; the model has no task woken outside any task, and the promise that a task in the queue never waits while a worker sleeps needs first its turn of fairness to follow the code |
| 1 | done 2026-11-08: `Worker`, lent to a closure by `worker`, which never inlines; `crates/tondo/tests/workers.rs` |
| 2 | done 2026-11-08: `tasks/` in 19 modules; the tools hear the scheduler through `tasks/tools.rs`, one word of bits; the replay, the exploration of orders and live builds' reloads, which change how it waits and what runs next, stay where they act |
| 3 | done 2026-11-08: `config.rs`, 37 variables read once as a program starts; the table of the toolchain's specification, 7.17, held to it by `crates/tondo/tests/environment.rs` |
| 4 | done 2026-11-08: one view of a string, `str_text`, and of an array's items, `array_items`, which seven helpers that copied bytes, some a byte at a time, gave way to; MessagePack and CBOR write their texts and read their input in place (encoding that record, 19 allocations to 10 and 33 to 28); one way back, `byte_array`, `int_array` and `string_array`, beside the view, of which `ints` had three copies; and `status`, how a native gives a `Result` with its value out, which `fs` and `io` each had and `process` wrote by hand. What `fs`, `net`, `http` and `sql` keep of a failure in their objects differs, a code with the system's number, one for each direction, a kind and its text, a text, and stays as it is |
| 5 | done 2026-11-08: lib.rs, 3,082 lines, is 239: strings, arrays, maps and sets, the builder, boxes, references, function values and the console are modules of their own, and raising a panic joined `panics.rs`; what other modules read of a string's, a map's or a reference's header is open to the crate |
| 6 | done 2026-11-08: debugger.rs, 5,209 lines, keeps the probes, their table and a run's history in 884, and `debugger/` has stopping, bindings and commands; diagnostics.rs keeps memory and the counters, and `diagnostics/` has dumps, the server of the tools and tracing; replay.rs keeps the mode, the baton the threads take turns with and starting, and `replay/` has the wrappers of C, which record and replay each call, and the recording on disk. Moving them showed the guard of thread-locals reading an indented block wrong: it listed three statics that are none |
| 7 | done 2026-11-08: a module of the standard library is the files of its directory, as any package's is (language 6.1), which the compiler had carried as one file; `http` is nine files, `sql` six (one for each driver), `grpc` six and `protobuf` five, the same lines of code and public API. They stay modules of the public API: the internal modules the plan named could not have the public types' methods, which only their own module declares |
| 8 | done 2026-11-08: the decoders of MessagePack, CBOR, TOML and YAML find an event's path only when a failure asks and compare keys in place, as JSON's does, and JSON's reader holds the string it reads rather than a copy (decoding a record of four fields, MessagePack 94 allocations to 19, CBOR 93 to 19, TOML 77 to 40, YAML 107 to 60, JSON 12 to 11; MessagePack 38% faster). JSON's writer takes a string as the UTF-8 it is, escapes by a table and starts with room for a small document: a small record encodes in 31% fewer instructions and 4 allocations, not 8. `std.model` knows each state by its JSON, half of what exploring a model costs, which that makes 20% faster; it keeps the text in memory, exact, rather than a fingerprint. The lock of the scheduler's queue is contended for 3.5% of spawning on 32 threads and hardly elsewhere, and stays; reading the clock was a fifth of a join, which now reads it once to schedule the task it wakes and notes when a task blocks on the coarse clock: 7% fewer instructions, 15% less time. The optimizer leaves no retain that the same block releases again in a model's code, and retains are 0.4% of the benchmarks: nothing to take there. `api::builtin_methods` read its text at every call, a third of checking a program that imports `std.http`, `std.sql` and `std.grpc`: a table now, 95 ms to 62 |
