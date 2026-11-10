# Speed after the refactoring: the compiler, waits, HTTP and the linker

This plan comes after `docs/refactor.md`. That plan reorganized the runtime and the standard library
and pinned their behaviour with tests. This one uses that safety net to make Tondo faster where the
refactoring did not reach: the compiler, which was outside it; the runtime's blocking waits, which
phase 8 did not touch; HTTP, which runs on a scheduler of its own; and the linker, which repeats the
same work on every build. The goals are the maintainer's:

- compile as fast as possible, because script mode compiles and runs in one command;
- few allocations and few cycles;
- every core in use where the work allows it;
- Release stays the fastest code, as always.

The maintainer approved the whole plan on 2026-11-08, the long-term parts included. The steps marked
**Decision** still go to the maintainer with numbers before any code is written.

## How this plan was made, and what to check first

It comes from a read-only review of `28805b99` on 2026-11-08, done while phase 0 of the refactoring
was closing. Nothing was measured: the machine was at load 55 with the mutation run, and the review
could not touch the tree. Every claim below cites the code at that commit. The numbers come from
`bench/baselines/pre-refactor.md`, measured at 31bd462, and from the phases' own reports. Line
numbers will have moved by the time you read this; find the code again before changing it.

The table at the end says where each phase is, and is kept current.

## Rules for every phase

The rules of `docs/refactor.md` hold here too. In short:

- Measure before and after. Phase A builds the instruments, and nothing else starts before it.
- A phase ends published, with its gate passed: fmt, clippy and the workspace's tests; the cases in
  debug, replay, live and static builds; `.github/scripts/local.sh`; a round on the M2; the Nightly;
  the models; and the baselines within 5%, with no more allocations.
- A commit that only moves code goes alone. A bug found on the way gets its own commit and its test,
  as before.
- A design decision gets its entry in `docs/decisions.md`. Nothing here should change a
  specification. A new environment variable, a new CLI flag or a change to the public API is a change
  of specification, so propose it to the maintainer first.
- **Same bytes.** Where a step says its output must not change, build every case with the `tondo`
  from before the step and with the one after it, and compare the executables byte for byte, masking
  the live build id. This was done for the parallel IR in 1af56dc with `/tmp/osr/same.sh`. That
  script is gone: write it again in `~/.cache/tondo-scratch`, or in `.github/scripts` if it earns a
  place there.
- The lessons of the refactoring still apply:
  - judge a flagged benchmark by `perf stat -e instructions:u`, because layout alone moves
    nano-benchmarks by up to 60%;
  - rebuild release before comparing;
  - never edit the tree while a gate runs;
  - check a command's exit status before `git commit`, because `cmd | grep` hides failures;
  - always run the check of a scripted split (`check_split.py`), because a script once dropped items
    silently.

## Order

| Phase | What | Needs |
| --- | --- | --- |
| A | Instruments: compile benchmarks, a linearity guard, network benchmarks against Go, profiles of waits | — |
| B | Cheap compile wins: the verifier, lints, `fmt`, `std` loaded in one round, `check`'s analyses in parallel | A |
| C | Blocking waits without allocations | A |
| H1 | HTTP: quick wins on the current design | A |
| D | Split `check.rs` and `codegen.rs`, moves only | — |
| E | Check only the bodies of `std` the program reaches | D |
| G | One core for the ELF and Mach-O linkers | — |
| F | Caches: `std` parsed once per process, Cranelift's code by function, the runtime's index for the linker; a **Decision** on a shared runtime for `tondo run` | E, G |
| H2 | HTTP, gRPC and TLS on the workers' poller | A, H1; a design the maintainer approves |
| I | The duplication and allocations left | — |
| J | The checker on every core, if the numbers still ask for it | B, D, E |

The order puts the cheap and safe work first, then what the measurements confirm, then the large
designs. G comes before F because F's linker cache would otherwise be written twice, once per format.

What is already done and must not be redone: parsing on every core; the parallel IR and Cranelift on
the pool (`parallel.rs`); the linker's mmap and its parallel reads and relocations; the table of
`api::builtin_methods` (phase 8); and the standard library loaded as multi-file modules with one
identity per file (phase 7).

---

## Phase A: instruments

### A1. Compile-time benchmarks

`bench/` measures only the runtime, while compile time is half of what a script costs. Nobody would
notice a regression of the compiler today.

1. **Fixtures**, under `bench/compile/`:
   - `hello.to`, which prints a line;
   - `server.to`, a `std.http` server with one route that encodes JSON;
   - `imports.to`, which imports `std.http`, `std.sql` and `std.grpc` (phase 8 measured its check at
     95 ms, then 62);
   - a project made by a deterministic generator in Tondo (`bench/compile/generate.to`): modules of
     records, enums, generic functions, closures, `match`, interpolation and calls into `std.json`,
     at about 3,000, 24,000 and 192,000 lines. The 24,000-line program is the size used in October;
     the 8× steps serve A2. `~/.cache/tondo-scratch/big60` is a 60,000-line project used for the LSP,
     if it helps to compare.
2. **Commands measured:**
   - `tondo check`;
   - `tondo build`;
   - a cold `tondo run`: an empty build cache, with the runtime's archives already extracted;
   - a warm `tondo run`: the cache hit, `exec` included;
   - the first build after installing, extraction included (about 110 ms in October), as a row of
     its own.

   Each command runs with its own `XDG_CACHE_HOME`.
3. **Measures:** the median wall time of N runs, and `perf stat -x, -e instructions:u` when `perf`
   is there (`perf_event_paranoid` is 2 on this machine, which allows counting user code). A runner
   in Tondo, `bench/compile.to`, writes the table, as `compare.to` does for the runtime.
4. **Where the time goes.** Add an internal switch that prints the time of each part to stderr: load
   and parse, `std`, meta/derive/templates, check, mono, lower, the MIR passes, building the IR,
   Cranelift, writing the object, and linking. `TONDO_COVERAGE_STD` is the precedent for an internal
   switch of the compiler. If the toolchain's table (7.17) must list it, ask the maintainer first.

### A2. A guard against passes that grow too fast

Three quadratic passes were found by hand so far: a block of 200,000 `let`s, `prove::Facts`, and the
LSP's highlighting. In the Nightly, check and build the generated project at N and at 8N, and fail
when the ratio of instructions passes about 10×. The failure names the part, using A1's switch.

### A3. Network benchmarks against Go

There is no HTTP, `std.net` or gRPC row in `bench/`. That is where H1 and H2 act, and where the ports
of Gin and dd-trace-go would run.

Add `bench/tondo/src/http_test.to` and the same names in `bench/go/bench_test.go`:

- `http/hello 1x1000`: one keep-alive connection, 1,000 GETs in a row to a server in the same
  process;
- `http/hello 64x100`: 64 client tasks at once;
- `http/json 64x100`: a POST of a small record, decoded and encoded;
- `http/tls 64x100`: the same over HTTPS, with a certificate kept in the bench project
  (`net.Tls.pem`);
- `net/echo 1x1000` and `net/tls echo 1x1000`: a round trip over `std.net`.

Go's side uses `httptest.NewServer` and an `http.Client` with `MaxIdleConnsPerHost` set to 64. Report
allocations per operation too. In Tondo the client is reqwest on the same tokio runtime, so a row
mixes client and server: use a profile to tell them apart. gRPC rows can come later, because Go's
side needs `google.golang.org/grpc` in `go.mod`.

### A4. A profile of the blocking waits

Before C, profile the four rows below to learn where their time and allocations go:

- `runtime/mutex 4x1000`: 15.69× Go, with 4,056 allocations per operation against Go's 6;
- `runtime/select 1000`: 1.28×, 4,355 against 4;
- `tasks/channel 10000`: 1.19×, 316 against 2;
- `runtime/join 1`: 5 against 2.

These are the figures at 31bd462. Use `tondo profile`, then
`TONDO_HEAP_RATE=512 tondo profile --memory` and `profile show --sample alloc_objects`, as phase 8
did, and `perf stat`.

Questions to answer:

- How many allocations does a contended lock make, and from where: the `Arc<Wait>` (C), `held()`'s
  `Vec`, `update`'s closure, or something else?
- How much of the time is parking, waking on another worker, and stealing?
- Do Go's four goroutines run in parallel at all? Run Go's side with `GOMAXPROCS=1` and with the
  default to see.

**What it found (2026-11-09, at c5cfe870).** The figures of 31bd462 measured `tondo test`'s watch of
lost updates, which kept where each lock was taken: `--bench` measures without it now
(docs/decisions.md). Without it, by median time and allocations per operation, against Go 1.27 on
the same machine:

| Row | Tondo | Go | Allocations, Tondo and Go |
| --- | ---: | ---: | ---: |
| `runtime/mutex 4x1000` | 76 µs | 21.5 µs, 15.3 µs with `GOMAXPROCS=1` | 29, 6 |
| `runtime/select 1000` | 149 µs | 147 µs | 4,355, 4 |
| `tasks/channel 10000` | 285 µs | 276 µs | 316, 2 |
| `runtime/join 1` | 303 ns | 329 ns | 5, 2 |

- The contended lock allocates little now: of its 29 allocations, 5 are `Wait`s and the rest start
  the four tasks. Its time is in `lock` and `unlock` themselves, 55%, and hardly any in parking
  (`work_until`, 3%). Go runs it faster on one processor: its four goroutines run one after
  another, while Tondo's four tasks take the lock from four workers at once.
- A channel's allocations are its `Wait`s, 96%: one each time a side blocks. C removes them.
- `select` allocates on every call, before it waits: 94% of its allocations are two `Vec`s, of the
  cases' locks and of their order, and two more when it blocks, the waiters and the `Wait`.
- `join` makes the `Wait`, the closure, the task and two in `join` itself, where Go makes 2.
- Reading the running task through `worker`, which never inlines (phase 1), takes 9% of the time
  of the lock's row and 11% of the channel's.

### A5. The baseline

Save the tables of A1, A3 and the runtime as `bench/baselines/pre-speed.md`, the same way as
`pre-refactor.md`, and keep the `tondo` of that commit in `~/.cache/tondo-scratch/pre-speed/` as the
reference binary for the gates.

**Done when** the four instruments run, A2 runs in the Nightly, and the baseline is committed.

---

## Phase B: cheap compile wins

Each step is small, the output must not change (except in B2 and B5, which say what to compare), and
each is measured with A1.

### B1. Cranelift's verifier is on in every build

`codegen.rs:906-908` builds Cranelift's flags without `enable_verifier`, whose default is `true`
(`vendor/cranelift-codegen/src/settings.rs`). `Context::compile` therefore checks the IR of every
function several times (`vendor/cranelift-codegen/src/context.rs`, the `verify_if` calls). Cranelift
describes this as slower compilation useful during development, and Wasmtime turns it off by default.

Set it from `cfg!(debug_assertions)`. The tests and the cases run `target/debug/tondo` and keep it on;
the release build that ships turns it off. That is the only flags builder in the compiler, but search
again before the change.

Proof: same bytes for every case (the verifier never changes code), and A1's check and build rows.

### B2. `lints::run` parses the whole project again for nothing

`lints.rs:98-104` parses every file of the program again. `tondo check` (`main.rs:255`) and the MCP
tool `check` (`mcp.rs:193`) always call it, even without `--docs` or `--performance`, when it has
nothing to look for.

Return early when neither is asked, setting `diags.asked` as it does now. When one is asked, parse the
files with `parallel::map` in the files' order.

Proof: the JSON of `tondo check` is identical across the CLI and MCP tests, and A1's check row of the
generated project goes down.

### B3. `tondo fmt` formats one file after another

`fmt_command` (`main.rs:1917`) reads, formats and writes each file in turn, and copies each text once
more (`text.clone()`). Format with `parallel::map`, then report and write in the order of the
arguments, so the output does not change. Compare with the text in the `SourceMap` instead of keeping
a copy.

Proof: the CLI's fmt tests.

### B4. `std` loaded in one round, without copying it

`stdlib.rs:283-311` loads the standard library in rounds. Each round parses, on every core, the
modules wanted so far, and only then learns what they import, so the next round waits for the
slowest file of the previous one. Each file's text is also copied into the `SourceMap`
(`text.to_string()`, `stdlib.rs:291`), and `SourceMap::add` scans it for its lines on every build
(`source.rs:82-85`).

The graph of imports inside `std` never changes for a given compiler:

1. Compute, once per process (a `OnceLock`, or in `build.rs`), each std module's direct std imports by
   reading its `import std.` lines, as `brings` (`stdlib.rs:203`) already does, with the same mapping
   as `source()` (`testing` → `fuzz`, companions first) and as `companions()`. Handle
   `import std.x as y`.
2. From the program's imports, compute `pending` in exactly today's order: round by round, file by
   file, with the same deduplication and the same `replaced` modules. The source ids, the bases and
   the order of the modules then stay as they are.
3. Add every file to `sources` in that order, and parse them all in a single `parallel::map`.
4. In debug builds of `tondo`, assert that the imports the parser found are those of the table, so
   that a mistake in reading the lines fails a test.
5. Let std files lend their text instead of copying it: `SourceFile.text` (`source.rs:30`) becomes a
   `Cow<'static, str>` or an `Arc<str>`. Compute a std file's line starts once per process.

Proof: same bytes for every case, the JSON of `tondo check` identical, and A1's hello and imports
rows.

### B5. `tondo check` of a project runs its analyses one after another

`driver::check` (`driver.rs:210-229`) analyzes the project for this machine's system, then for each
other system it runs on when it has files of a system, then once more with its tests. Each is a full
parse and check.

Run them on scoped threads (`std::thread::scope`) and merge the results in today's order. The
driver's state is per thread: `OPEN`, `ROOT` and `SYSTEM` (`driver.rs:143-150`). Copy what the caller
set into each thread, through `with_open`, the root and `for_system`.

The pool of `parallel.rs` serves one caller at a time (its `busy` flag), so the second analysis runs
its own maps on its own thread. That is acceptable, because the serial checker is the largest part
anyway. Measure both ways.

Proof: identical diagnostics, order included, in the CLI, LSP and MCP tests; the per-system cases;
and A1's project row.

### B6. `deps::resolve` several times per command

`deps::resolve` has ten call sites. `inputs()`, `checked_systems()` and `analyze_with()` each read the
lock file and every manifest again, and the function may rewrite `tondo.lock`.

Keep its result within one run of a CLI command, keyed by directory and `update`, and never across an
LSP or MCP session, where `tondo.toml` changes under it. Do this only if A1's breakdown shows it.

**Done when** B1 to B5 are published with their proofs. B6 is done, or found not to matter, with the
number that says so.

---

## Phase C: blocking waits without allocations

### C1. Why

Each blocking wait makes an `Arc<Wait>`: `Wait::new` (`tasks/waits.rs:35-37`) has 19 call sites,
among them the contended lock (`sync.rs:258`), the channels (4), join, group, select, io (3), the
clock, contexts, live builds (2) and http. The rows of A4 are the worst against Go, and phase 8 left
them as they were.

The `Arc` exists because some holders outlive the wait:

- **timers:** `Timer` holds an `Arc<Wait>` (`tasks/mod.rs:255`, `tasks/timers.rs:34`); a timer can
  fire after the task has gone on, and stale timers are dropped lazily (`timers_ended`);
- **wakers inside futures:** `io.rs:115` (`Notify`), `http.rs:137` (`Done`), and the clock in
  `clock.rs`.

Every other notifier is an entry the task removes from a queue itself before it goes on, as the doc
comment on `Wait` says.

### C2. Design

1. **Confirm with A4** that the allocations are the `Wait`s. If `held()` (`sync.rs:186`), `update`'s
   closure or anything else allocates on each operation, fix that first.
2. **One `Wait` per task.** A `Wait` kept in the `Task` (or in the frame of the waiting function)
   serves waits whose notifiers are all queue entries the task removes before going on, and that
   register no timer. That covers locks and `RwLock`, `updateWhen`, barriers, channels, `select` over
   channels, joins and groups, `join` and `group.wait`. The task already points to its current wait
   (`Task.waiting`, `tasks/mod.rs:162`), and cancellation and reloads claim it through that pointer
   (`tasks/reload.rs:22`). Check that a task never has two waits at once, for instance a wait inside
   a cleanup that runs while it is blocked.
3. **Waits that keep the `Arc`, for now:** a timer (a timeout, or the task's `within` deadline, which
   `DEADLINE` notifies), or a waker. If A4 shows that timed waits matter, recycle their `Wait`s
   through a free list in the `Worker` of phase 1. A recycled `Wait` needs a generation, packed
   together with the operation's index into `fired`, so that a stale timer cannot claim its next use.
4. **The invariant to write down and to model.** Today's code reads a `Wait` after claiming it:
   - `sync.rs:330` and `sync.rs:683` read `(*w).task` after `claim`;
   - `tasks/channels.rs:173` and `tasks/channels.rs:186` claim through a waiter;
   - `tasks/reload.rs:22` claims through `t.waiting`.

   This is safe only because a task never moves past its wait before the claimer's notification. A
   `Wait` that lives in the task or its frame depends on the same rule. Write it in `waits.rs`, check
   it in the models (Wake, Lock, Select, in `testing/models` under `std.model`, and TLC while it
   remains), and read `task` before the CAS wherever that costs nothing.
5. **The scheduler's share of the 15.7×.** If A4 shows the gap is parking and waking across workers
   rather than allocation, measure how long a woken waiter takes to run, and whether `SPIN_ROUNDS`
   suits, before changing any policy. Go may simply run the four goroutines one after another.

### C3. Proof

- the models;
- `tests/orders`, and `--schedules all` on the sync cases;
- `TONDO_CASES_REPEAT=100 cargo test -p tondo --test cases cases` on the M2;
- `.github/scripts/mutants.sh` over `tasks/waits.rs`, `sync.rs` and `tasks/channels.rs`;
- the rows of A4: allocations near Go's count, and time by instructions and by wall time.

**Done when** locks, channels, `select`, joins and groups without timers allocate nothing per wait,
and the gate passes.

---

## Phase H1: HTTP, quick wins on the current design

### What happens today

`http.rs:20-45` builds a tokio runtime with `worker_threads(2)`. The HTTP server and client, gRPC,
`std.net`'s TLS (`net.rs:669-672`, `net.rs:1156`), file watching (`files.rs:439`, `files.rs:582`) and
`io.rs:123` all run on those two threads. TLS, HTTP parsing and HTTP/2 framing therefore use two cores
at most.

One request goes through these steps (`server.rs:827-870`):

1. The method, path, query and authority are copied into `String`s, and the version is
   `format!("{:?}")`.
2. Each header becomes two `String`s.
3. The `Request` is built with several `Arc` and `Mutex` values.
4. It goes through an `mpsc::channel(1024)` (`server.rs:734`) to the single Tondo task `serve`
   (`std/http/server.to:356`). That task takes it with `tondo_rt_http_next`, through a receiver
   behind an `Arc<tokio::sync::Mutex<…>>` (`server.rs:32`), and starts a task for it.
5. That task calls `requestHead`, splits a tab-joined text into seven strings
   (`std/http/server.to:370`), and calls `requestHeaders`.
6. The reply goes back through a `oneshot` (`server.rs:859`).

Each request crosses threads twice and makes a few dozen allocations.

### Steps

1. **Size tokio's workers by A3:** try `available_parallelism`, half of it, and 2, at 1 and at 64
   connections. Add no new environment variable.
2. **Read the request's head without joining and splitting text.** Use natives for each field, or one
   native that fills the record's fields. The method and the version become static strings. The
   headers stay in hyper's `HeaderMap` inside the native request and turn into Tondo strings only
   when the program asks (`requestHeaders`, or one header by name). The public API does not change;
   `std-api.txt` guards it.
3. **Count the allocations per request** before and after with A3.

### Proof

The http, grpc and net cases; the observe tests (requests in dumps, `flight`, `HTTP_SERVER_ACTIVE`);
`local.sh services`; the M2; and A3's rows.

---

## Phase D: split the compiler, moves only

The refactoring split the runtime and `std`, but not the compiler:

- `check.rs` has 18,065 lines, with one `impl Checker` of about 16,500 lines from line 1288 and about
  400 methods. `call` alone runs from line 15765 to 16540.
- The `Checker` struct (`check.rs:844-1063`) has about 120 fields, many of them for one std module
  each: `duration`, `instant`, `datetime`, `fs`, `process`, `pacing_module` and others.
- `codegen.rs` has 10,044 lines.

`check/` already has `indexing.rs` and `routes.rs`, and `codegen/` has `cabi.rs` and `worker.rs`.
Splitting is a prerequisite of E and J, and it makes a cache by module possible later.

**`check/`**, by the methods' current order (adjust the boundaries while moving):

| Module | What | Lines at 28805b99 |
| --- | --- | --- |
| `program.rs` | `program`, tests, fuzz, bench, snapshot, recover, `within`, panics | 1289–2236 |
| `std_checks.rs` | `is_std_fn`, model names, regex and template literals, reflection, numbers, intrinsics, the clock | 2236–2916 |
| `foreign.rs` | externs, exports, C types, layouts and calls | 2916–3216, 4148–4188, 4702–4770, 14176–14271 |
| `declarations.rs` | prelude, imports, module names, types, generics, kinds, defaults | 3216–4148, 4188–4243, 4770–4944 |
| `annotations.rs` | annotations, rules, messages, `embed` | 4243–4702 |
| `traits.rs` | signatures, traits, impls, `satisfies`, bounds, inference from bounds | 4944–6108 |
| `owners.rs` | receivers, owners of methods, resolving types | 6108–6594 |
| `functions.rs` | `new_cx`, `function`, ownership, `script_main`, unused results, closures, `defer`, `spawn` | 6594–7688 |
| `natives.rs` | sync, time, process, io, file, `Ref`, atomic, channel and handle methods; `lost_updates` | 7688–9340 |
| `statements.rs` | declarations, blocks, statements, assignment, places, loans, `for` | 9340–10841 |
| `expressions.rs` | coercion, `expr`, record and union literals, unification and inference | 10841–11843 |
| `contexts.rs` | contexts and their initial values | 11843–12169 |
| `patterns.rs` | `match`, `select`, patterns, `with`, update, construction | 12169–13035 |
| `literals.rs` | `try`, map, set and array literals; numbers, templates, display and formatting, queries, strings, paths, unknown names | 13035–13325, 14756–15765 |
| `calls.rs` | generic, method, language, trait and dot calls; function values; cursors; hints; `call` and its arguments | 13325–14756, 15765–17285 |
| `operators.rs` | unary, arithmetic, binary, `if` | 17285–17828 |

**`codegen/`:**

- the runtime's declarations (`Rt`, `runtime_signature`);
- glue;
- the entry point and `process_main`;
- the debug and dump tables;
- live codegen;
- coverage;
- `FnCodegen` grouped by the statements and values it builds.

**Technique:** that of phases 5 and 6 (`~/.cache/tondo-scratch/move_lib.sh`, `move_sub.py`). Each
file is an `impl<'a> Checker<'a>` block; the fields and the helpers that other files use become
`pub(super)`; one module per commit; and phase 7's comparison of the multiset of code lines after
every move.

**Proof:** same bytes for every case, and A1 unchanged. `lsp.rs` (6,161 lines) can follow the same
way later; it is not needed for E.

---

## Phase E: check only the bodies of `std` the program reaches

### Why

`check.rs:1435-1437` checks every declared body, those of `std` included. Then `mono`
(`mono.rs:26-83`) keeps only what `main`, the tests, the exports, the contexts and `Display` reach.

Every program loads `prelude` and what it imports (`formats`, `math`, `panics`, `reflect`,
`serialization` and the `std.internal` modules), several thousand lines, and checks all their bodies
on every build. A program that imports `http`, `sql` and `grpc` checked in about 62 ms after phase 8,
against about 25 ms without them. The standard library comes with the compiler, and its own tests
check it.

### Design

1. **Declarations as today:** every signature, type, trait and impl of every loaded module.
2. **Bodies through a work list.** First every body that is not std's: the program, its tests and its
   dependencies, which may have errors. Then each std body when something reaches it, until nothing
   new is reached.
3. **What reaches a std function:**
   - a checked body that names it: a call, a method call resolved to it, a function value;
   - for a trait method, conservatively, the impls of that method among the loaded ones;
   - mono's own roots: the `Display` of `main`'s error, the contexts' maker, log, label and
     computed functions, the exports, the `debug.View` views, the conversions of `?`, derived impls
     and iterator defaults;
   - what the checker or the code generator call by name: the uses of `is_std_fn` (`model.*`,
     `cli.markup`, `format.apply`), `panics.report`, the loops of testing and fuzz, templates.

   Build this list from `mono.rs` and `codegen.rs`, not from this plan.
4. **An unreached std function** keeps its signature and a body marked unchecked. Mono stops with an
   internal error that names any unchecked function it instantiates, so that a missing edge fails a
   test loudly instead of compiling wrong code.
5. **State that bodies feed, to review one by one:**
   - `dropped_calls` and `only_computes`;
   - `cell_uses` and `lost_updates` (which compares every write with every read, std's included
     today);
   - `calls`, `test_calls`, `spawning` and `test_spawns`;
   - `const_uses` and `public_fns`;
   - `model_steps` and `checks_models`;
   - `contexts` and `context_of`;
   - the index;
   - the numbering of closures: `closure_base` stays, and closures take their ids in checking order,
     which is deterministic but differs from today's.
6. **What stays eager:**
   - the editor's analyses (`analyze_for_editor`, the index);
   - documentation fences (`analyze_fence`, which replaces std modules);
   - `TONDO_COVERAGE_STD`, whose measure counts the std lines that compile;
   - an internal switch that forces everything to be checked, for differential tests.

### Proof

The cases in all five modes; every case built eager and lazy with its output and exit compared; the
deterministic builds of `tests/link.rs`; the API snapshot and the guide's tests; A1's hello, server,
imports and generated rows. The bytes may differ from today's, because types are interned in another
order. The program's behaviour must not.

---

## Phase G: one core for the two linkers

`link/elf.rs` has 2,469 lines and `link/macho.rs` 2,715, and they repeat their core:

- `archives()` (`elf.rs:785-843`, `macho.rs:747-803`) differs in three lines: ELF brings a member
  only for a *required* undefined global, and the two return differently;
- `intern`, `add` and much of `take`;
- `redirected` (`elf.rs:247`, `macho.rs:288`), the same table with Mach-O's `_` prefix;
- `error`;
- `Image` with `put`, `u32`, `u64` and `read32` (`elf.rs:1585`, `macho.rs:1156`);
- `uleb` and `sleb`;
- the skeleton of the work list that keeps live sections.

Windows (item 12 of the wish list) would add a third copy.

**Steps:** move the shared parts to `link/core.rs`:

- the table of globals and its interning;
- loading archive members, with the format's `parse`/`take` and its rule for which globals bring a
  member as parameters;
- `Image`;
- the redirect table built from `RECORDED` with the platform's prefix.

Moves first, each commit alone, then the unification with parameters. Layout and writing stay per
format.

**Proof:**

- same bytes for every case on Linux;
- on macOS, the M2 round and the checks of a cross-built Mach-O used in H12 (structure, binds,
  rebases and `__unwind_info`, read with `llvm-objdump`);
- `tests/link.rs`;
- the Nightly.

---

## Phase F: caches

### F1. `std` parsed once per process

The LSP analyzes the whole project after each edit, and MCP's `check` and `fix`, `run --watch` and
`test --watch` analyze it again and again. Each time they parse the same std files, which never
change for a given compiler, together with their derived `Display` (`stdlib.rs:313`).

**What blocks it:** spans are offsets in one `SourceMap`, and a file's `base` is where the previous
one ended (`source.rs:82-83`). A std file's spans therefore depend on what was loaded before it.

**Steps:**

1. Give std files fixed bases: a region at the start of the `SourceMap`, laid out in the order of
   `SOURCES`, whatever the program loads, with the program's files after it.
2. Keep the parsed std modules in a `OnceLock`, and clone or share them into each analysis. Measure a
   clone against a parse.
3. This is the first piece of the cache by file that phase 7 prepared for, and that the maintainer
   expects to come.

**Proof:**

- the LSP tests, including go-to-definition into std, which writes std's sources to disk;
- the source links of dumps and traces;
- identical diagnostics;
- the latency of `~/.cache/tondo-scratch/lsp5/latency.py`, before and after.

### F2. Cranelift's code, kept by function

`compile_one` (`codegen.rs:1597-1670`) compiles every function on every build. The vendored Cranelift
has the feature `incremental-cache` (`vendor/cranelift-codegen/Cargo.toml:73`, which needs serde,
postcard and sha2), with `Context::compile_with_cache` (`incremental_cache.rs:39`) and the trait
`CacheKvStore` (`incremental_cache.rs:100`).

The key hashes a function's IR and the ISA's flags. Tondo's source locations are numbers of sites
within each function (`codegen.rs:5964-5966`), so they do not shift from one build to the next.
Editing one function of a script, or building two programs that share std, should then recompile only
what changed.

**Steps:**

1. Enable the feature, and check `cargo build --locked` and the vendored build with the new
   dependencies.
2. Keep the store in `cache_dir()/code/<compiler identity>/`, as files sharded by key or as one pack
   per build with its index. Write it after `compile_all`, not from the pool's threads. Sweep it
   after 30 days like the other entries, with a cap on its size.
3. Call `compile_with_cache` in `compile_one`.
4. Check that a cached result keeps everything `compile_one` reads:
   - the source locations;
   - the call sites and their exception handlers;
   - the patchable call sites and the nop units;
   - `code_info`;
   - the ranges of value labels that dumps read;
   - the unwind information.

Live builds hash the code they get (`code_hash`), which works the same way with a cache hit.

**Proof:** same bytes for every case built with a cold cache, with a warm one, and without it; A1's
rows after editing one function of the generated project, and after building `server.to` twice with
a different `main`; and the hit rates.

### F3. The runtime's index for the linker (after G)

On every build the linker:

1. reads the runtime archive's symbol table into a map (`elf.rs:788-797`);
2. brings members in rounds, parsing each member's ELF to learn what it needs next
   (`elf.rs:802-841`).

The archive is the same for a given `RUNTIME_HASH`.

**Steps:**

1. Record, for each member, the globals it defines and those it needs. Do it in `build.rs`, embedded
   next to `runtime-hash`, or once beside the extracted archive in `cache_dir()/runtime/`.
2. Resolve the closure of members from that record, without parsing them.
3. Parse the chosen members in one `parallel::map`.
4. Keep the same rules: the first definition in archive order wins, weak symbols, and ELF's
   `required`.

**Proof:** same bytes for every case in both formats, and A1's build rows.

### F4. **Decision:** `tondo run` against a shared runtime

If A1 still shows linking as the largest part of a cold `tondo run` after F3, what remains is
relocating and writing the runtime's code on every build: a program that prints a line is 2.6 MB.
Linking `run`'s builds against a shared object of the runtime, kept in the cache, would leave only
the program's object to link.

What it costs and touches:

- start time, because the loader relocates a large object (compare with 1.35 ms today);
- the redirection of the C library's calls in recorded runs (`redirected`, `elf.rs:247`);
- whatever reads the runtime's addresses: the trace table, dumps, the debugger and unwinding;
- live builds;
- `--static`.

Bring the numbers and a recommendation to the maintainer before writing any code.

---

## Phase H2: HTTP, gRPC and TLS on the workers' poller

### Why

`docs/decisions.md` (2026-10-23, "The network without a thread per connection") measured that the
hop from tokio's reactor thread to a worker kept `std.net` behind Go: a round trip of 22 µs against
12. Polling from the workers (`poll.rs`) later removed that hop for plain TCP. HTTP, gRPC and TLS
still take it, on two threads (H1).

### Design to propose to the maintainer, with its entry in `docs/decisions.md`, before code

- **hyper inside Tondo tasks.** Run hyper's connection futures in Tondo tasks, polled with the
  mechanism `std.net`'s `Ready` streams already use: the task polls the future in place with a waker
  that notifies it.
- **IO and time:** IO through `poll.rs`'s `Registered<mio::net::TcpStream>` (`attempt`, `ready`),
  implementing `hyper::rt::Read` and `Write`. hyper's `Timer` on the runtime's timers. hyper's
  executor, which HTTP/2 uses for its background futures, as Tondo tasks owned by the server.
- **TLS** with rustls's sans-IO connection on the same socket. This replaces tokio-rustls and
  `std.net`'s two piping tasks (`net.rs:669-672`).
- **The rule of 2026-10-23 stays:** no callback crosses into Tondo, Tondo calls natives. The Tondo
  side changes shape: a task for each HTTP/1.1 connection asks the native for its next request and
  calls the handler itself, and HTTP/2 gets a task for each stream. `serve`'s single queue goes
  away. The public API does not change; the snapshot guards it.
- **Recording and replay.** Today a recorded run drives tokio from its worker (`http.rs:31-37`,
  `drive` at `http.rs:55`). With the sockets on the workers' poller, recording goes through the
  wrappers that `std.net`'s plain TCP already uses.
- **Order of work:**
  1. the HTTP/1.1 server;
  2. HTTP/2;
  3. TLS;
  4. WebSocket upgrades;
  5. files, ranges and uploads to disk;
  6. compression;
  7. the draining stop and SIGTERM;
  8. the gRPC server, then gRPC-Web and Connect;
  9. the client, on hyper's client connections instead of reqwest. Check feature by feature what
     reqwest gives today: redirects, proxies, cookies, compression, multipart;
  10. the gRPC client;
  11. tokio itself, once nothing uses it (file watches, `io.rs`, `process.rs`).

  Each step is measured with A3, and no old path stays behind as a silent fallback.

### Proof

- every http, grpc and net case;
- the observe tests (requests in dumps, `flight`, counters);
- the debugger and a replay of a server case;
- `local.sh services` and the gRPC interop tools;
- the M2 and the Nightly;
- A3 against Go at 1 and at 64 connections, and the allocations per request.

---

## Phase I: the duplication and allocations left

### I1. One debugger session for `tondo dap` and `tondo debug`

Each keeps its own session with the program. Both send a command, read the answer, treat its
`error`, and keep `break`, `delete`, `catch` and `steps` for a replay (`dap.rs:271-282`,
`debug.rs:617-640`). Give them one session type in `observe.rs`, or in `debug.rs`; MCP already goes
through `tondo debug --json`.

Proof: the dap, debug and MCP integration tests.

### I2. Text conversions on hot paths

The runtime still has 89 `from_utf8_lossy` and 37 uses of `str_text`; phase 4 replaced only part of
them. Go through those on hot paths, where a native copies a Tondo string into Rust and then back:
the HTTP request (`server.rs`, `http.rs`), gRPC, `process` and `fs`.

Proof: A3's allocations per operation.

### I3. YAML and TOML

Decoding a record of four fields takes 60 allocations from YAML and 40 from TOML, against 11 from
JSON (phase 8's numbers). TOML decodes through toml_edit's `DocumentMut` (`toml.rs:9`, `toml.rs:55`),
and YAML through saphyr's tree together with its parser's events (`yaml.rs:14-15`). Profile them as
phase 8 did. A reader of events without the tree is the larger option; take it only if the numbers
justify it.

---

## Phase J: the checker on every core, if the numbers still ask for it

After B, D and E, measure check on the generated project. If checking bodies still dominates large
projects, bring back the design of 2026-10-26 with the numbers: a fixed number of contiguous groups
of bodies, each group's new types interned again in order, and the HIR renumbered. The maintainer
deferred it then because of its permanent cost; the split of D makes that cost lower. This is the
maintainer's decision.

---

## Status

| Phase | State |
| --- | --- |
| A | done 2026-11-09, but for A2's first night. A1: `bench/compile.to`, with `--old` and `--guard`, over the programs of `bench/compile` and the projects of `bench/compile/generate.to`, and `TONDO_COMPILE_TIMES`. A2: `--guard` in the Nightly, which found at once two parts that grew as the square of a program's types: `mono` looked at all of a trait's implementations for each type, 3.4 s of checking 192,000 lines and 0.39 s now, and the table of a build's trace looked for each name among all the others, 67 s of building them and 3.4 s now; both builds keep every case's executable byte for byte. A3: `http/` and `net/` against Go, in bench/tondo/src/http_test.to and bench/go/http_test.go. A4: what it found is under A4; first that `--bench` measured the watch of lost updates, which the maintainer agreed to leave out. A5: bench/baselines/pre-speed.md, and the `tondo` of c5cfe870 in `~/.cache/tondo-scratch/pre-speed/` |
| B | done 2026-11-09, against bench/baselines/pre-speed.md, every case built to the same bytes. B1: Cranelift checks what it compiles in tondo's debug build alone, 25% fewer instructions to build the generated projects. B2: `lints::run` parses only for `--docs` or `--performance`, on every core, 10% fewer to check them. B3: `tondo fmt` formats on every core, the 549 cases in 9 ms, not 52, and says and writes the same. B4: the standard library in one round, the imports of each of its files read from its lines before its first declaration, which debug builds hold to the parser: hello's 6.1 ms to load it are 3.6; its text is still copied and its lines counted, at most 1% of checking `imports.to`, not worth a type of text the whole compiler reads. B5: `tondo check` analyzes the other systems and the tests on threads of their own: with B2, checking 3,575 generated lines takes 85 ms, not 137, and 24,995 lines 490 ms, not 829. B6: `deps::resolve`, 0.01% of checking a project, stays as it is |
| C | done 2026-11-09. A task waits with its last wait again when nothing else holds it, as a timer or a waker does until dropped, which needs no wait of its own beside the task and keeps the `Arc` for those; waits.rs says why no queue points to it then, which Select's `Gone` checked and Lock's `Gone` now does, in TLC and `std.model`. `select` keeps its lists on the stack up to eight cases. Allocations per operation: `select 1000` 4,355 to 26, `channel 10000` 316 to 14, `join 1` 5 to 4, `mutex 4x1000` 29 to 25, the rest those of the tasks and channels a run makes; time within noise, mimalloc being quick already. Its proofs found what it did not cause: two threads of one `tondo check` wrote files aside under one name since B5, and a retiring worker woke no other when it left no task, so that the last to sleep, which had counted it busy, never saw the deadlock left (once in a hundred runs on the M2, and in `local.sh nocc`). cargo-mutants mutates neither an `unsafe fn` nor `let … else` chains: of what it could, the dedup of `select`'s locks survived the cases, whose tasks seldom race, and a unit test holds it now. Left for the loop against Go: an uncontended `update` costs 15 ns where Go's lock and unlock cost 3.6, mostly a retain and a release of the mutex around the call and reading the running task several times |
| H1 | done 2026-11-09. Tokio's workers are half the cores, two at least: with 64 clients at once `http/hello` takes 89 ms, not 156, `http/json` 94, not 159, and `http/tls` 136, not 196, against Go's 114, 114 and 83; one client as before. A request's head is read a part at a time from hyper's, and its headers stay in hyper's map until the program asks for one: 157 allocations a request of `http/hello`, not 189. Half of what is left is the client in Tondo, whose `Response.text` lowers every header's name to find the charset, and routing splits the path of each request: for I and the loop against Go |
| D | done 2026-11-09. `check.rs`, 18,065 lines, is 1,564: the checker's methods are in 16 modules of `check/`, by what they check, as the table above has them. `codegen.rs`, 10,055 lines, is 3,151: the runtime's functions, glue, live builds, the tables a program keeps, its entry, and `FnCodegen` in ten modules by what its code builds. One module a commit, moved as they were by scripts that count the lines before and after, and every case builds to the same bytes |
| E | done 2026-11-09. `check/reach.rs`: the bodies of the program, its tests and its dependencies, then each body of the standard library that a checked one could lead monomorphization to, found in the checked HIR as `mono.rs` walks it: calls and function values; for a trait's method, the implementations for the receiver's head and the trait's default, all of them for a type parameter, an opaque result or a union; the `Display` of what is shown and of what it holds; the error of a failing `main` and of a context's initial value; the helpers of a union's `Decode`. An unreached body is a placeholder in `Program.unchecked`, and `mono` stops with an error of the compiler if it is asked for one: removing the edge of `main`'s error does that on `tests/cases/run/main_displays_std_error.to`. An editor's analysis, `TONDO_STD_WARNINGS` and the internal `TONDO_CHECK_STD` check every body; the cases pass with `TONDO_CHECK_STD=1` too. `TONDO_COVERAGE_STD` needed no such switch: it counts what compiles, which is what mono reaches either way. Against `tondo-h1`, by instructions: `tondo check` of hello −36% (127.7 M to 81.5 M, all but nothing of the 82 M left when no body of std is checked), of server −23%, of imports −23%; `tondo build` −4% to −12%; the generated projects within 1%, the 192,785 lines +0.6% for walking their HIR once. A first version looked at every implementation of a trait for each call on a type parameter and doubled that row, which A1's `--old` found and a table by trait, method and head fixed |
| G | done 2026-11-09. `link/core.rs` holds what the two linkers repeated: `Image` and `error`; bringing archives' members in, `load_members`, in rounds read on every core, with the trait `Members` for what differs, which global brings a member (in ELF only one some reference requires) and how the format takes one in; the redirects of recorded functions, `Redirects`, with Mach-O's `_` and the names libSystem imports some functions by; and interning a global. `uleb` and `sleb` were Mach-O's alone. `take`, the work list of live sections, sections in ELF and atoms in Mach-O, the layout and the writing stay each format's: what they share is the shape, not the code. Four commits, each one move or one unification, 11 lines fewer; every case builds to the same bytes against E's `tondo`, on Linux normal, `--static` and `--debug`, and on the M2 normal and `--debug`, where `tests/link.rs` and the cases pass too; `local.sh` passes `musl` and `nocc`. Comparing Mach-O needs one output name and one cache for both builds: the ad-hoc signature holds the file's name, and the executable the path of the runtime's archive |
| F | F3 done 2026-11-09: beside each archive of the runtime in the cache, the first link writes the index of its symbols (`link/core.rs`, `index_symbols`: a table of 131,072 slots of the hash of a name, where it is in the archive and its member, 2 MB for the 36,000 symbols and 4.2 MB of names of the runtime), and later links look each name up in place instead of reading them all; the index goes with its archive. Every case builds to the same bytes on Linux, normal, `--static` and `--debug`, and on the M2 normal and `--debug`. `tondo build` of hello takes 569.0 M instructions, not 585.4 M, and links in 41 ms, not 45; of server and imports 1–2% fewer. Bringing the members in at once without reading them, F3's steps 2 and 3, would need a second `take`: which global a member defines depends on comdat groups taken before it, on the shared libraries, and on the redirects of a recordable build, so the rounds stay; reading the 668 members of the runtime every build costs 4 ms and taking them in 10. F1 measured: a deep copy of std's parsed modules takes 25–30% of parsing them on one core (hello's 6 files, 1.0 ms against 4.0; imports' 49, 4.7 against 16.4), and the parse already runs on every core, so it would save 1–2 ms of an editor's analysis. The maintainer's decisions of 2026-11-09: F1 left as these numbers say; F2 later, since it saves only what a build compiles again, Cranelift being 0.2 ms of hello's build, 9 of server's and 0.5 s of the 24,995 lines' 2.7 s, and it brings ten crates; F4 not taken: the static link gets faster instead, where hello's 41 ms go, taking in the runtime's 668 members 10 ms, writing 9 and resolving and collecting 7 |
| H2 | the design above approved by the maintainer on 2026-11-09, with its entry in docs/decisions.md. Step 1, the HTTP/1.1 server, done 2026-11-09, with what of HTTP/2 and TLS the server needs so that no request takes the old path: the server's sockets are the workers' poller's, and hyper's connection is a future the workers poll at their turns, which a waker only puts in their list (`tasks::poll_soon`, `turn_host`), as the code that wakes it may hold a lock its poll takes; the task of each connection takes its requests, answers one of HTTP/1 itself and starts a task for one of HTTP/2; TLS goes through tokio-rustls over the poller's socket, with no thread of tokio's; a body read as a stream waits with its task. Against I2's `tondo`, two rounds each, with 64 clients `http/hello` takes 28 ms, not 88 (Go's 114), `http/json` 47, not 91 (Go's 114), and `http/tls` 118, not 137 (Go's 83); one client 73, not 87 (Go's 43). Found on the way, each with a commit of its own: a poll inside `wait_for` could set tokio's context in another thread's variable, a stream let go inside a poll waited there, moving the task to another thread, and, closed between a poll and its wait, took the wake meant for that wait; a connection woken while another thread let it go was never polled again, which hung HTTP/2 over TLS on macOS 7 times in 30; and a notifier that found a task notified already did not write its state while the task took a notification with a plain store, so each could miss the other, a race older than H2 that its wakes of finished waits made hang HTTP/2 over TLS once in 20 runs. Step 2, HTTP/2, done 2026-11-09: the future hyper's HTTP/2 starts for each stream is polled at the workers' turns too, as the connection is (`Turning`), and the connection keeps them (`Streams`, hyper's executor); `http/tls` takes 113 ms, not 118, with 3% more allocations, a future and its waker for each stream where tokio made one. Step 3, TLS, measured 2026-11-09 and left as it is, as the design allows: with `perf` on `http/tls`, rustls, the cryptography and tokio-rustls's adapter take 1.2% of the time, the adapter 0.2%; the row's time is the kernel (15%), h2 (12%), allocations (12%), locks taken by another thread (7%) and the client, reqwest with tokio's threads and the parsing of each request's URL (17%). hyper's timer on the workers' timers, tried the same day and left out: hyper sets a timeout for the head of each request, and each one in the workers' heap, under the lock the scheduler takes, cost `http/hello` 10% with 64 clients and 6% with one, with an allocation more per request, so hyper keeps tokio's timer until step 11 gives the host's sleeps a timer of their own, with a lock of its own. Steps 4 to 8 found the server's WebSockets, files, compression, draining and gRPC on the poller already, as step 1 moved the server whole; step 5's blocking work, files, uploads, binding and a child's pipes, went to the runtime's pool from tokio's the same day. And a first part of step 9: the client's requests are polled in place by the program's task, not as tasks of tokio's, so one client takes 56 ms, not 72 (Go's 43), with two allocations fewer for each request, while 64 clients over TLS take 115, not 113. Left for the next steps: hyper's timer and the server's timeouts are still tokio's, and the client is still reqwest, whose connections run on tokio's threads, which is most of what one client and TLS cost now |
| I | I2's HTTP done 2026-11-09, by the allocations of `--profile memory` on A3's rows: a client response keeps hyper's map of headers and reads its head when asked; its charset is read with two searches when it is UTF-8's or none; a client's URL is joined with one copy; a route is matched before its map of parameters is made; the empty string, HTTP's common methods and its versions are kept for good, as literals are. Allocations per request, against G's `tondo`: `http/hello` 157 to 109, `http/json` 193 to 158, `http/tls` 163 to 115; time within 5%, the hop to tokio's threads being what costs, which is H2's. I1, I3 and the rest of I2 not started |
| J | left on 2026-11-09 by the maintainer's decision: after E, checking hello's bodies takes 2 ms and 24,995 lines 0.5 s, and the checker's state is shared and mutable |
