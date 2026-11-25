# What is pending

One list of the work that is open: decided and not done, measured and not
fixed, or waiting for a decision. It is the plan: the work follows [its
order](#the-order), and what is not here is not being done. The other lists
say what Tondo is and why: [the roadmap](roadmap.md) what each milestone
covers, [the speed plan](speed.md) its phases, [the decisions](decisions.md)
why things are as they are, and [the benchmarks' page](../bench/README.md)
each row against Go. This one says only what is left, so that nothing
agreed is lost between them.

How it is kept:

- A piece that lands takes its line out in the same push. A thing found on
  the way, or agreed in a conversation, gets its line the day it is found.
- A line says what, since when, and where its detail is. It is checked
  against the code before it is written and before it is repeated: a note
  of what was left to do outlives the doing.
- A row behind Go leaves this list when a measure puts it at or under Go's,
  or when the maintainer accepts its reason, not before.
- The order is changed here, with its reason, when a measure or the
  maintainer says so. Work that is not next in it waits or is said why not.

Last gone through on 2026-11-23, at the end of a day's work that stopped
where each line below says, so that the next session starts from this page;
on 2026-11-25 the three pull requests of another session were added to it.

## The order

What is done next, and after it, with the reason for each place. A step's
detail is in the tables below. The inliner's weighing of step 2 comes
before the chain's rest because the chain's adapters wait for it too.

1. **What is owed**: what is left of the cost the scheduler's pieces added
   to a spawn and a join, and what the Nightly's jobs under emulation showed
   on 2026-11-23, five tests that fail on x86-64 and two jobs that do not
   fit their time. First because they say whether what is on main is sound,
   and anything they find is fixed before more is built on it.
2. **`trim`**: the inliner weighing a call by what inlining buys and a
   region for the copy, which the plain form in Tondo, measured again,
   waits for; then the list of what stays behind Go. Second because its rows
   are the furthest behind, and the inliner's weighing is a piece many other
   calls wait for. The weighing is by trial, as the maintainer asked on
   2026-11-23: the budget is a ceiling, a call past today's 31 is opened
   and kept only where the passes after it make something of it, and left
   as it was where they do not. Where it stands: the design is written and
   a first build exists, on main as it was before 2026-11-23's changes;
   its first numbers were void, read beside a change to `thread` that
   compiled wrongly. Next, in this order: the benchmark's own loop given
   back its code and `thread` following copies pushed (Owed now), then the
   trial carried onto main with both and with `prove`'s mend in, and
   measured. These measures hold this machine for hours and do not wait
   for step 1's jobs under emulation, whose work is done on the runners
   beside them; a fault found there in code a release program runs goes
   ahead of them.
3. **The chain's rest**: the loop's fast arm after its branch, which is
   also what the chars rows lack, and the reads of the task's attention.
   Then a new table against Go, at the baseline and at x86-64-v3. Here
   because the diagnosis is done and fresh, and the table is what orders
   the steps after it.
4. **What the examples and the rows left pay**, by one round of profiles
   taken again: the library functions, `url.encoded` first, the map copied
   in a loop over it, and programs/sum lines and queries/countBy in the
   same hour, which is the same work with the same tools. The examples'
   are a larger share of real programs than any row left, and the round
   says which rows texts reach.
5. **The views of texts**: their two numbers, which steps 2 and 4 give,
   then the maintainer's decision.
6. **An array of tasks waited for as one wait, then `parallel for` and
   `.parallel()`**: the largest piece, after what it stands on.
7. **The tightening**: texts' counts at direct sites, the header at 16
   bytes, Phase K, and what the speed plan left of the network and of the
   runtime's text conversions.
8. **What is left of the maintainer's list of 2026-11-14**, in the table
   of the optimizer's program below: each technique the steps above did
   not carry is tried on the rows and measured, and its numbers are kept
   whether it stays or not. His word of 2026-11-23: every kind of
   optimization, not the inliner alone.
9. **What the maintainer said yes to on 2026-11-23**, after all of the
   speed work by his word of the same day, where it had been after the
   inliner: the orphans of a program that is a container's first process,
   `std.quic`'s first piece, measured, and the archive read on demand.
10. **A service shown end to end**, last, by the maintainer's word of
    2026-11-23: every row at or under Go's is the promise kept first.

Beside the order, wherever a piece leaves a wait: the tests that fail now
and then, when they show; the heap's checks on musl and macOS after
their first clean runs; the scheduler's open mutant; what each worker runs
in the text of `tondo blocked`; the examples of `tondo explain`; and the
manual's unheld quotes and listings.

## Owed now

| What | Since | Detail |
| --- | --- | --- |
| What is left of the cost the scheduler's pieces of 2026-11-20 and 21 added to a spawn and a join, each within the bar alone. Reading the clock straight and asking for the workers with a load took 6.2% of the instructions of `runtime/join 1`, now under the table's count, and about 1% of `tasks/spawn 1000`, `select 1000` and `channel 10000`. Left: `ffi/waits from a C thread 100`, still 3.5% over the table, whose count moves with the turns of the waits' spin locks and which is to be told from them first; the readings of the clock that remain at each spawn and join, some seven, for which the processor's own counter is to be weighed before any stamp becomes a count, so that every wait keeps its meaning; and the wakes added for a task woken next to a busy worker, each to keep the stall it fixed as its reason or to go | 2026-11-22 | AGENTS.md, "A change to the optimizer…"; [decisions](decisions.md) |
| A read that can wait without end is not ended by a deadline or a cancel when it goes through a thread of the system: a file opened by its path that is a pipe, a FIFO, a terminal or a device. A `fs.readText` of a pipe written after a second gave its text at a second under a deadline of 50 ms. The pipes of a process that was started wait on the poller since 2026-11-22. For those files, a thread of the stream that reads only when the program asks, as the console's input has, and their open as work a deadline abandons; writes to them with it. `Process.cancel()` still waits for the pipes it captures | 2026-11-22 | Spec 11.15 and 11.17; `Mode::Host` |
| What the Nightly's jobs under emulation showed on 2026-11-23, the first night they ran whole: the tools' directory was all that had failed 106 tests the night before. Neither job fits its time. The cases' binary, 862 cases, was cut twice at its hour and a half on arm64 with children three seconds old, slow and not hung, and three of the job's four and a half hours went there; on x86-64 it was 88 minutes in when the job's limit cut it, after the manual's examples had taken 42. Owed for that: a watch that tells slow from hung by what the binary last started and not by a clock, and the cases in shares over jobs of their own. Five tests failed on x86-64, each to be read to its cause: `a_library_that_tondo_built_ends_and_unloads` and `a_contract_crosses_to_a_library_and_to_a_program_alike` abort with `memory allocation of 51 bytes failed` in a program that loads a library; `cpu_has_answers_from_the_level_or_the_processor` takes what it expects from `/proc/cpuinfo`, which under qemu is the arm64 host's; `a_dump_stops_a_task_that_computes_at_its_next_line` finds no value of the local in the dump; and `vectors_at_the_level_of_avx2` gave its words at last, `tensor_definitions.to: its build: timed out after 1200s`. On arm64 none of the 241 tests that ran failed, and observe's binary, with `a_cpu_profile_of_a_program_that_waits_says_it_waited`, was not reached; on x86-64 that test passed Under way, on the branch `nightly-watch`: a watch that takes a test binary for hung when the processor time of its tree has been the same for ten minutes, an end the script reaches by itself before the job's limit, with what was running, a line of progress from the cases' binary, and a limit for a case's build read from the environment. Its first run, 45240500984, with the cases whole and the build's limit as today, was past three hours with no cut when this was written. To read in it: how far the cases get and in how long, the builds that take more than two minutes at the processor's third level, each binary's time, and whether the two aborts and the dump's test fail again there. Then the cases in parts over jobs, part k of n on each architecture with every n-th of the other binaries, n from those times; the three commits of the branch that are for main land, and the two that only cut the Nightly to the emulated jobs do not; and the two aborts are found on the runner | 2026-11-23 | Nightly run 45176074860, jobs 137697536743 and 137697536798; trial run 45240500984 |
| The readers of bytes or text from outside that have no search in the Nightly, two lots left of three. The first is in: a GraphQL document, the forms of a request, its cookies and a query's parts, and it found three ways for a document of GraphQL to end a server, mended with it. Left: the images' decoders and their EXIF, certificates and keys in PEM and DER, and CSV; then the decoders of `std.encoding`, a UUID, the parsers of dates and durations, the streams of gzip and zstd, and gRPC's frames; MySQL's packets last, as no public call reaches their reader. Each search runs some minutes before it lands, and what it finds is mended with it | 2026-11-22 | `testing/fuzz` |
| Eight of the Nightly's searches change their input blind: what they try is the runtime's own code, of which the search's coverage sees nothing. They are the ones of JSON, YAML, TOML, MessagePack, CBOR, protobuf, the regex and a request's form in parts; the images' decoders and the inflaters, which have none yet, would be the same. A runtime built for `--fuzz` with the sanitizer's counters over its Rust, read by the search beside its own, gives all of them eyes at once: a piece of days, after the inliner's weighing. Until then each such search says in its own comment that it is blind | 2026-11-22 | `testing/fuzz`; `crates/tondo/build.rs` |
| The reader of a request's form in parts written in Tondo, on its own merits: measured against the runtime's at a megabyte first | 2026-11-22 | `httpNative.requestForm` |
| A benchmark's measured loop, since `testing.bench` gives its closure's value, which the maintainer keeps as it is (2026-11-25) on one condition, that it adds no cost and no noise to what is measured: a benchmark is to be stable and exact. So this comes before the next table against Go is read: `map filter sum 10000` and `numbers map filter sum 10000` run one instruction more for each element, 383,522 where they ran 373,588, 2.7% over the table, with the same sources and no assert in them. Not `prove`: in the test, larger now that the closure is called from two places, one of the loops opened there reloads a value from the frame at each turn. To do: name that loop and its reload, then try the run that gives the value after the measured ones, and the measured loop compiled apart from its test, on the sources of 8044d9ff, and keep the one that gives 373,588 back; the first changes the specification's "la primera" and five places of the manual, the second moves every row's base at once and waits for the next table against Go. A program's code did not change with it, read for `examples/kvstore`; `examples/logstat` is to be read too | 2026-11-23 | `Checker::bench` in `crates/tondo/src/check/program.rs`; the compilers of 6015f2fb and 662246a4 on the same sources |
| What the mend of `prove`'s widening left unread. `sortedBy 10000` measured 5.8% slower with 0.3% fewer instructions and 5.2% more cycles, and it is the mend's doing and not where the code lies: its `mergeSort[(Int, Order)]` has fewer instructions in each of its three copies, 1,559 where it had 1,623, and why fewer instructions take more cycles is not found. The row is at 1.39 ms where Go's takes 3.89. To do: each side alone in four orders of the code, and the loop of `mergeSort` read on both sides for what the dropped checks changed in it. A build of the server does 1.4% more instructions with it, in the same time. And what `prove` says of every case that runs, on both sides, for a proof it had and lost, was not run: the rows lost none. `prove` has no way to keep a check it drops as an assertion, which would hold a wrong range by a run and not only by the answers the tests assert | 2026-11-23 | [decisions](decisions.md): the entry of `prove`'s loop behind unsettled code |
| `thread` following a copy to what it was made of, measured and not pushed: on main as it was at 03c77299 no row does more than 0.1% more instructions; `iter filter sumBy 10000` does 23.9% fewer with its time the same and 5.2% more cycles; `echo 1x1000` makes none of its 900 heap allocations and `tls echo` 4,000 of its 4,900, into regions; seven of twelve executables are smaller, by up to 1.3%, and two larger, by up to 0.7%; a build takes the same time; every row's test passes on both sides. Before its push: its tests on top of `prove`'s mend, the rows it moved keeping their sign, and each side alone for the row whose cycles rose | 2026-11-23 | The branch `follow2` |
| The work of another session on speed, three pull requests stacked on main as it was at f591f2a3, read on 2026-11-25 and taken in pieces by the maintainer's word. From the first, `perf-quick-wins`, 24 commits: its own layer of TLS 1.3 records does not go in, as a layer of security is used and tuned and not written again here, and it took an alert in plain text for a clean end; its own HTTP/1.1 client does not go in either, as the work is to tune `hyper`'s use, its read buffer first, and the bytes that stay are a row's reason; a count of those that inspect a C thread's task is read as one bit where it waits for them, which would free the task under a second one, though it cannot happen today, as `tasks()`, the one place that pins, holds every lock of the tasks' table while it does: the waits are to read the count, with that rule written beside them and a test that pins twice; its server's own savings, the headers' maps used again, small bodies in line and the `Host` kept, went out with the client and can come back as a measured piece, as they tune the use of `hyper`; `workers_seeking.to` hung once on arm64 with that branch's weaker orderings in the scheduler, to find by a stop and mend; and what is left, in the optimizer, the generator, the library and the runtime, goes through the gate and the whole rule against main, each change with its entry in [decisions](decisions.md) and the rows it moves, before any of it merges. The second, `b3-trial-inline-trim`, is the weighing by trial of step 2 written without a bound on what a caller may grow by: a build of `tensor_definitions.to` takes 10.0 s where it took 2.3, in the code generator, with `main` at 4,474 blocks for 2,478; to redo on the first once that is in, with the bound, and compared with the trial this list already held. The third, `b3-string-views`, a text's bytes lent to a view in the frame, is saved as a draft as it was left: every reader of a text's layout outside the runtime, the two numbers of step 5 and the maintainer's decision come before it | 2026-11-25 | Pull requests 51, 52 and 53; none went through the gate nor the whole rule |

## Rows behind Go

The table of 2026-11-21 has 60 of 75 rows at or under Go's time.

| Row | Behind by | What is left | Since |
| --- | --- | --- | --- |
| texts/trim, texts/trim read | 3.4×, 3.25× | The inliner weighing a call by what inlining buys, and a region for the copy of a function over its budget that returns its parameter or what it made; then what still stays behind Go, by function and with its machine code, as a list for the optimizer. `trim` in Tondo, measured again on 2026-11-22, takes 558 instructions against the native's 325, with a heap allocation, for those two causes and no other; its form waits for them | 2026-11-20, [decisions](decisions.md): "Trim stays a native until a call is weighed by what inlining buys" |
| queries/numbers iter map filter sum | 275.0k instructions against 142.8k, with no count for a parameter's parts (285.1k without) | The loop's fast arm placed after its branch; the two reads of the task's attention an element, once it is said which check they are; the address of that flag held in a register through the loop; and where the total lives, to read again in the machine code | 2026-11-21 |
| queries/iter filter sumBy, queries/countBy | 1.71×, 1.18× | The same loop's rest; `countBy` by its own profile | 2026-11-21 |
| texts/chars ascii, texts/chars other | 2.24×, 1.54× | Two taken jumps a byte against Go's one, which the fast arm after its branch lifts; and the loop's count checked for overflow each turn, which is Tondo's to keep unless it is proven | 2026-11-21 |
| programs/sum lines | 2.40×, 5 allocations against 2 | Its profile first: the reader and `readLine`'s path. Its lines are short texts, which nothing copies | 2026-11-21 |
| processes/true by name, by path | at Go's time, 0.98× and 0.97×; 474 allocations against 59 and 456 against 34, three times the bytes | The environment copied whole at each start, twice, and built again as the child's: some 300 of the allocations with 55 variables. The child's environment prepared once and made again only when the program changes a variable; nothing passed at all when a start adds nothing, so the system's own is used. And the lookup of a program by its name on the child's `PATH`, in since 2026-11-22: measured again on 2026-11-23 with both sides of the comparison in one environment, it costs a start by name 5.24% more instructions, 175.7k against 166.9k, and 21 allocations, 475 against 454, where 2.8% was written, the comparing tool's extra variable having hidden the rest. It makes three allocations for each directory of `PATH` it tries, two joins and the text for the check: one buffer for all of them takes those back. Its time and cycles, which fell and are why it was taken, measured again in one environment on a quiet machine | 2026-11-22 |
| http/hello, http/json, in bytes | 3.15× and 1.5× Go's bytes a request, 20.4 KB against 6.5 and 22.3 against 14.7, with fewer allocations than Go, 49 against 69 a request in `hello` | The memory profile by function of `hello 1x1000` sees 41 of the 49 allocations and 3.9 of the 20.4 KB: about 8 allocations and 16.5 KB a request, four fifths of the bytes, are in none of its lines, though the benchmark's counters count them. So first which allocations the profile misses, [a hole of the tool](#tools-and-tests), then whose they are. A lead and no more: two blocks of 8 KiB a request would be 16,384 of those bytes, and 8,192 is what hyper first gives a connection's read buffer. Of what the profile does see, a third is the headers' map, 1,312 bytes in 4 allocations, then the client's response, 559, and the server's request, 480. The bench page gives `json`'s number with no reason and `hello`'s not at all. The bound on a response adds 24 of these bytes, 0.1% | 2026-11-23 |
| net/tls echo | 1.03–1.05× | rustls's buffers, an allocation a record each way, which its API without buffers would not make | 2026-11-12, [speed plan](speed.md), H2 |
| ffi from a C thread, texts/toUpper other, tasks/spawn, runtime/mutex 4x1000, programs/count words | under 1.10× | No piece yet. The first and the third do 3.2% and 2.3% more work than at the table, by the clock read at each spawn and join: [owed](#owed-now) | 2026-11-21 |
| The table at x86-64-v3 | of 2026-11-15, before a week of the optimizer's work | Taken again with the next table | 2026-11-14 |
| A new table against Go | | After the chain's two pieces, the fast arm and the attention's reads | 2026-11-21 |

## Speed work decided and not done

| What | Since | Detail |
| --- | --- | --- |
| A worker put beside a held one is retired on its next look once the held task leaves its worker, even while tasks wait and the one worker left is held again at once; the watcher then puts another beside 100 ms later. A thread started for nothing, and up to 100 ms more for a waiting task: small and rare, seen on 2026-11-23 by a stop that held `main` past the watcher's 100 ms and made the worker beside start late. To weigh, not decided: the worker owed back taken only when no task waits, or put off until a worker idles. It changes the protocol the Wake model and the checker hold, so its model first | 2026-11-23 | `lets_one_go` in `crates/tondo-runtime/src/tasks/work.rs`; `watch_held` in `tasks/foreign.rs` |
| The bound on a response read whole costs each request of the client 24 bytes, three words kept with it: how much more it may give, the same for the decoder's sink, and the bound the request carries to its response. The sink's is held even by a response with no `Content-Encoding`, which has no decoder. To try, with the next piece that touches the client: the sink's word in the decoder's own state, measured on `http/hello` | 2026-11-23 | `crates/tondo-runtime/src/client.rs`, `Received`; measured by the memory profiles of the rows |
| Texts that share another's bytes where a result does not leave the turn (views): two numbers first, what `trim` in Tondo leaves on its rows and how many texts of over seven bytes made by `slice`, `trim` and the pieces of `split` die in their turn in the examples; then the maintainer decides. The loans measured on 2026-11-19 paid for a word read by every text; this form has not been built | 2026-11-22 | [decisions](decisions.md): "Parts of texts lent were built, measured and set aside" |
| The library functions the examples' profiles named: `url.encoded` (39.9% of linkcheck's instructions), `ffi.fromWide` and `ffi.wideUnits` (7.4% and 6.5% of profiler's), `splitOnce`, `fs.exists`, sql's bound parameters, the regex iterator, and a search for one byte paying the set-up of a search for many | 2026-11-19 | Profiles of the examples at f365a7da; to measure again before each |
| A map copied whole at each write inside a loop over it: 54% of chat's allocations. The loop goes over a snapshot that holds the map, and each write through the loan copies it; the first shape, a parameter a function takes, is done | 2026-11-19 | Profiles of the examples at f365a7da; the two ways to lift it were to be weighed and never were |
| A part stored into a value that leaves its function, copied out where it leaves (logstat) | 2026-11-19 | With the views |
| Texts' counts at direct sites, first of the tightening; a text's header at 16 bytes | 2026-11-19 | [speed plan](speed.md), Phase K |
| The inliner weighing a call by what inlining buys | 2026-11-20 | Reopens `trim` |
| An archive read on demand. Today `archive.open` reads the whole file, inflates every entry of a zip and the whole stream of a `tar.gz` or `tar.zst`, and holds twice what it inflates: 2 GiB for a megabyte made for it. It is to take the index alone, a zip's and a plain tar's by reads at an offset and a compressed tar's by one pass that keeps the headers; inflate an entry when it is read, up to 128 MiB when read whole; write each entry as it inflates it in `extract`, up to 8 GiB and a million entries; and give an entry as a stream with `Archive.reader(name)`. With few allocations, as every reading on demand: an entry's stream fills the caller's buffer, and its cases and a row against Go count them | 2026-11-22 | After all of the speed work, by the maintainer's word of 2026-11-23 (step 9); `crates/tondo/std/archive.to` |
| A process's `output()` and `wait()` wholly on the poller, its pipes and the child's end, which takes out the reader that outlives its call and what keeps it safe | 2026-11-22 | `crates/tondo-runtime/src/process.rs` |
| An array of tasks waited for as one wait | 2026-11-20 | |
| `parallel for` and `.parallel()` in a chain, after the three above | 2026-11-20 | README, Status |
| Phase K: regions tightened, the alignment of loops' heads, and the rows whose time moves by a fifth with where their code lies | 2026-11-18 | [speed plan](speed.md), Phase K |
| gRPC's client on the workers' poller; what still uses tokio: hyper's timer, the server's timeouts, the WebSocket client | 2026-11-10 | [speed plan](speed.md), H2, steps 10 and 11 |
| One debugger session for `tondo dap` and `tondo debug`; text conversions on the runtime's hot paths; YAML's scanner | 2026-11-09 | [speed plan](speed.md), I1, I2, I3 |

## The optimizer's program

The maintainer's list of 2026-11-14, with what the compiler does of each
today, read on 2026-11-23 from the passes' own headers, the generator's
rules marked `Tondo:` and the rows, and corrected by the compiler's
session against the code, and what is left to try. A technique
is tried on the rows, measured by the whole rule, and its numbers are
kept in [decisions](decisions.md) whether it stays or not. Owed before
step 8 starts: the techniques in the order the rows behind Go ask for
them, with the rows each is expected to move; three came out of the
reading against the code and are in the table, a load that nothing in a
loop writes marked so that the generator takes it out, checked arithmetic
merged and taken out, and a switch in the MIR.

| Technique | Today | Left to try | With |
| --- | --- | --- | --- |
| Inlining | Small functions, by a cost of 31 and three rounds, and a copy of a function for the `fn` value it is given (`inline.rs`) | The weighing by trial; a profile deciding | Step 2, PGO |
| Escape analysis, the frame | Texts and closures in the frame (`stack.rs`); what a function makes and lets go, in a region (`regions.rs`); records, tuples and enums as a local for each field (`split.rs`) | Collections that start in the frame and move out when they grow; what a region cannot take, by the rows | Steps 3 to 7 |
| Tail calls | None: no call returns for its caller | The maintainer's decision first: whether a call in tail position reuses its frame always, on request or never, and what a stack and a `defer` show then | A question for him |
| Constants folded and passed on | The generator's; a closure's captures that are one constant wherever it is made (`captured.rs`); the ranges of integers (`prove.rs`) | A parameter that every call gives the same value, in a function that is not opened | Step 8 |
| Dead code | The generator's; a value read only by tests already decided is not made (`thread.rs`); checks proven never to fail (`prove.rs`) | Nothing known: a row decides | |
| Common subexpressions | The generator merges values with no effect, and its reading of what a store can change takes out a load repeated with no store or call between | A length or a field read twice across a call the MIR can see through; checked arithmetic, which the generator takes for an effect and does not merge | Step 8 |
| Strength reduction | A division or a remainder by a power of two as a shift or a mask where the dividend is proven not negative, and by any other constant as a product by its inverse; a product by 3, 5 or 9 as one `lea`, and a value plus a sum with a constant as one; `x * 2.0` as `x + x` | An index times a size as a sum that grows with the loop | Step 8 |
| SIMD | `Vector[T]` at each processor's width up to 256 bits; the arithmetic of float arrays | A plain loop over an array made of vectors by the compiler; 512 bits | Step 8 |
| Instruction scheduling | None | Not proposed, as processors reorder: only if a row shows it | |
| Loop unrolling | A counted loop with a short body, four turns at a time (`unroll.rs`) | When and by how much, by the rows; a loop of a small constant count opened whole | Step 8 |
| Invariants out of loops | The length a loop reads each turn (`hoist.rs`); the generator takes out only values with no effect: no load leaves a loop by it, as a load is not marked as one nothing writes, and checked arithmetic counts as an effect | Loads the MIR knows nothing in the loop writes, marked so; checked operations whose inputs do not change; the checks a loop keeps | Steps 3, 8 |
| Loop fusion | A lazy chain, opened, is one loop with no array between | The eager chain, which still builds its arrays, in regions | Step 3 |
| Loop interchange | None | Inside `Tensor`'s own code, by its rows | Step 8 |
| Loop unswitching | None of a test. What there is, is two copies of a loop run four turns at a time, chosen by a test before it: one for a counter and a bound within 2³¹ of zero, whose overflow checks `prove` drops, and one that keeps them (`unroll.rs`) | A test that does not change in a loop, taken out with a loop for each side | Step 8 |
| If-conversion | In the generator, a pass of Tondo's own (`tondo_select.rs`): a branch whose two ways compute a few integers, with no memory and no call, chooses with a `select`, at most 6 instructions a way and 10 for both; and a `select` for `min`, `max`, `clamp`, saturating arithmetic, an index from the end and a division's guard | Ways that read memory, floats, and the limits by the rows whose branches miss: the chars rows, the chain's arm | Step 3 |
| Alignment and layout | Measured, with `--layouts` and `TONDO_LAYOUT_SEED`; nothing decided | The alignment of loops' heads; the functions' order by what calls what | Step 7 |
| Profile-guided optimization | None | A profile deciding what is opened, where code lies and which side is cold, when the others stop paying | Step 8, last |
| Link-time optimization | A program and the standard library compile as one; the runtime is linked as it was built | The runtime's small functions opened in the program's code, texts' counts first | Step 7 |
| Block placement | The calls of a panic and of an error are cold blocks, after the others | By a profile | PGO |
| Checks of bounds and of overflow | Dropped where the ranges `prove` holds show they cannot fail, at a loop behind code that had not settled too since 2026-11-23; the flags of an overflow that only a branch reads are not made into a value | What a loop still keeps, by the rows | Step 8 |
| Tests decided by the way in | A short block whose test each way into it already decides is copied into each way, and the test becomes a jump (`thread.rs`) | Following a copy to what it was made of: measured, waiting for its push (Owed now) | Step 2 |
| Copies passed on | `a = value` then `b = a`, with `a` read nowhere after, is `b = value` (`elide.rs`) | Nothing known | |
| Room before a loop | An array that a counted loop pushes to once a turn takes its room before the loop (`reserve.rs`) | A fixed number of pushes a turn, which takes that many times the turns; a push under a test only with a bound on the room it may take beyond what it fills, as the pass declines it today so that no array takes much more room than its elements | Step 8 |
| A copy of generic code for each type | One of each generic function for each type it is used with (`mono.rs`) | Copies that end the same merged, which saves size and not time | Step 8, last |
| Pairs of counts removed | An owner's reference taken, lent or borrowed where it was counted (`moves.rs`, `lends.rs`, `borrowed.rs`, `takes.rs`) | Texts' counts at direct sites | Step 7 |
| A value with one owner changed in place | A parameter the function takes (`takes.rs`) | A map copied whole at each write in a loop over it | Step 4 |
| Short texts without the heap | Up to seven bytes | Texts that share another's bytes | Step 5 |
| Texts searched with vectors | The runtime's searches, through `memchr` | Validating UTF-8, `split`, a letter's case: by the texts' rows | Step 4 |
| A `match` as a table | A chain: each arm in turn, a text's by a call that compares it, an integer's or a character's by a comparison; the MIR has no switch, so no table of jumps is made | A switch in the MIR: a table of jumps for integers close together, and for texts a test of the length or of the first word before the bytes | Step 8 |

## The library, decided and not done

| What | Since | Detail |
| --- | --- | --- |
| The orphans of a program that is a container's first process. A command's own children go to the first process when that command ends, and a Tondo program reaps only what it started, so they stay in state Z for as long as it runs. By the maintainer's word of 2026-11-23 the runtime reaps them by itself when it is pid 1, as .NET does: it looks first and takes only a child that none of its tables holds, and a program that is not the first pays nothing. What no shape can tell from an orphan is the child of a C library reached through a foreign call, whose own wait would then fail: the manual says so. Its case runs as a container's first process; the design before the code | 2026-11-23 | Seen with a case's static build as pid 1 of a plain `alpine` container; the runtime waits for each child by its pid today |
| QUIC, HTTP/3 and WebTransport, beside the HTTP/1, HTTP/2 and WebSockets the library has, in three pieces by the maintainer's word of 2026-11-23. `std.quic` first, a measured piece on the workers' poller with `quinn-proto` over the rustls the runtime has, whose numbers say what the other two cost. Then HTTP/3 in the server and the client, where the choice is between the young `h3` and the protocol's framing written here. Then WebTransport beside `WebSocket`, which follows a draft that still moves. Each with its tests against other implementations and its rows against quic-go, as Go's own library has none ready; the design of each before its code | 2026-11-22 | Read on 2026-11-23: `quinn-proto` 0.11.19, `h3` 0.0.8 and `h3-webtransport` 0.1.2 of May 2025, the draft's seventeenth of November 2026, in Chrome, Firefox and Safari 26.4 |
| A service shown from one end to the other, with no change to the language: an example that reads PostgreSQL and asks two servers at once, and the hard cases of it held as tests. A proposal left with the maintainer on 2026-11-23, read against the code and not run. Its three parts: errors of several kinds passed on by `?` and turned into a reply once, at the edge; what cancelled what said by the tools, who asked, why, a deadline, a sibling's error, a client gone or a stop, where it was seen and when the task ended, which `tondo blocked` and the trace do not say today; and the database under a cancel, a wait for a connection that times out, a query or a transaction cut, a stop that drains. Read in `std.sql`: a plain query gives its permit and its connection back after the wait and not in a `defer`, which by the permit's own rule and by not reaching the idle list should lose nothing, with no test against a real server that says so; no kind of error says that a `COMMIT`'s outcome is not known; and a query that was cancelled goes on in the server. The tests wait on what started, never on a sleep | 2026-11-23 | After the speed work, by the maintainer's word; `crates/tondo/std/sql/databases.to`, `Database.through` and `Database.transaction` |

## Tools and tests

| What | Since | Detail |
| --- | --- | --- |
| `tondo optimizations` says of a value that it "goes in a region that the caller of `f` gives" and not which region that is in the copy it is read in: one that ends with each turn of a loop, or the function's, which keeps every turn's until the function ends. It is the question an author asks of that line, and on 2026-11-23 it took a run of a million turns under the regions' check to answer it for one case. To name the region where the copy makes it known | 2026-11-23 | `tests/cases/run/regions/opened_loop.to`; `crates/tondo/src/regions.rs` |
| A build can take all of a machine's memory when its functions are very large. Seen on 2026-11-23 with a scratch compiler, never on main: the inliner's budget at 120 and its rounds unbounded ended after 22 rounds, and the passes after it then took 57 GB on the bodies it had made, until the system's guard ended the build and what ran beside it, three times. Two things owed. The inliner has no budget of growth for a caller nor for a program beyond its two constants, 31 statements and 3 rounds, and the copies made for `fn` values: the weighing of a call brings one, a count of statements a caller may gain, with the remark that says when it is reached. And a lead, not measured: a pass after inlining whose memory grows faster than a body's size, which a generated function of a program could meet too; which pass, found with one large function under a bound of memory | 2026-11-23 | `crates/tondo/src/inline.rs`; `optimize` in `crates/tondo/src/driver.rs`; scratch builds and measures run as units of their own with a bound of memory |
| A reduction of a failing input starts a worker for each of its rounds, which is what a slow machine pays: on 2026-11-23 the search's test failed once on x86-64 macOS with an input saved as far as the 30 seconds let it get, and a stop that made each worker take two seconds made the same red here at will. It is mended by taking out runs before single characters, 10 or 11 rounds for that input where there were 26 to 29, and the report says when the time ended a reduction. What a start costs under emulation no log says. If the report shows reductions cut there, the larger shape: one worker kept across the rounds, which changes the worker's protocol | 2026-11-23 | `reduce` in `crates/tondo/src/runner/fuzz.rs`; [decisions](decisions.md), "A reduction takes out runs" |
| A memory profile misses allocations that a benchmark's counters count: on `http/hello 1x1000`, with every allocation sampled, 8 of the 49 a request and four fifths of their bytes. To find which, by thread or by path, as a block grown in place or a thread that is not a worker; and a case that holds a profile's totals to the counters' | 2026-11-23 | `tondo test --bench --profile memory`; [the row](#rows-behind-go) |
| observe's `debugger_goes_back_in_a_replay` fails now and then under load. A copy says its probe since 2026-11-21: the line it prints at its next failure is the lead | 2026-11-16 | `crates/tondo/tests/observe.rs` |
| Which pipe had closed when SIGPIPE ended `tondo dap` on x86-64 macOS, in four of the seven runs of main up to 2026-11-22 (observe's `every_debugged_program_s_end_reaches_the_editor`: "the adapter had exited with signal: 13 (SIGPIPE)", 66 ms into a round). The commands that serve a client or a child for long now ignore that signal and take the write's error, so the adapter no longer dies of it; but its socket to the program cannot raise the signal, so the write was to a pipe that is not yet named. The test's report says the write's error at its next failure, if that was not the end of it | 2026-11-20 | `crates/tondo/tests/observe.rs`; CI run 44991205995, job 137112438117 |
| The code a benchmark's row reaches that the other Tondo makes differently is named only when its count of instructions changed: a function whose machine code changed with the same count, as the workers' loop with another allocation of its registers, is not on the list, and it was what moved `ffi/waits from a C thread` | 2026-11-22 | `tondo test --bench --compiler` |
| A leak's report that names what leaked: `TONDO_RT_CHECK_LEAKS` gives one number, and the hunt of 2026-11-22 needed a runtime built for it to learn that the object was a join. It is to keep each counted object's kind and the place that made it, and print the ones left at exit, at no cost when the check is off | 2026-11-22 | `crates/tondo-runtime/src` |
| The heap's checks see nothing of what the runtime keeps on lists of its own to use again, as freed joins: to the allocator they are alive. A runtime built with the checks is to give them back to the allocator, so a stale use of one faults | 2026-11-22 | `TONDO_RUNTIME_HEAP_CHECKS` |
| `tondo size --against` moves some hundred bytes between the program's data and the runtime's when only the runtime changed, summing to nothing: it gives data to a symbol, keeps one symbol of those at one address and counts bytes without a name as the runtime's | 2026-11-22 | `tondo size` |
| A function of Tondo that a thread of C calls runs on that thread's own stack, with no switch, for speed: half a megabyte on macOS, where XML's or YAML's decode at 128 levels takes more. So the bounds on nesting hold in a task, a handler and `main`, and not there. To measure, on the rows of calls from C, what a switch to a stack of a task's size costs a call; the manual says meanwhile to give deep work a task | 2026-11-22 | `crates/tondo-runtime/src/tasks/foreign.rs`, `tondo_rt_callback` |
| A program started by its name is started by a copy of the whole process, where one given by its path is started without one: the runtime sets the child's environment, its PATH among it, and the library under it then cannot look the name up for the child and falls back to the copy. On macOS the copy costs with the address space set apart: 3 to 7 ms plain, and 238 to 247 ms under the regions' checks, as the cases run, against 1 ms for a path either way. The runtime is to look the name up on the child's PATH itself, as the system does, and start every program by its path: a millisecond for all, in a release build too, and the macOS jobs lose a quarter of a second for each process a case starts | 2026-11-22 | `crates/tondo-runtime/src/process.rs`; measured on arm64 macOS |
| `tests/cases/run/profile_tasks_sleeping.to` showed two tasks nowhere in a thousand snapshots, once, in the musl job of 2026-11-22, and none in 720 runs of the same build under load here. Its code did not change that day. A task that a worker took off the queue and has not yet run shows with no stack, and the snapshot asks again a thousand times, 100 µs apart, and then gives what it has: a worker the system leaves unrun longer than that leaves the task nowhere. A stop that holds a worker there past the snapshot's tries would make it show every time; then a bound that waits for the worker's own step and not for a count of tries | 2026-11-22 | `crates/tondo-runtime/src/inspect.rs`, `snapshot`; CI run 45057663140, job 137316383127 |
| A script with no `#!` line is started by a program built with glibc or on macOS, whose C library hands it to the shell, and refused by one built static with musl, whose library does not. The runtime, which now finds the program itself, is to give one answer: when the system says the file is not in a format to run, start the shell with it, in every build. It needs a stage's streams kept for a second try, three more calls a start, to weigh on the row of a process's start beside the environment's mend | 2026-11-22 | `crates/tondo-runtime/src/process.rs`; `tests/cases/run/process_lookup.to` |
| A test that fails leaves the programs it started, and a test binary then may not end, as under emulation on 2026-11-22. Of the 69 tests that start a program, 62 start at least one outside the guard that ends it, and the guard ends only its own child, not what a `tondo run` started in turn. The guard is to give each started program a group of its own and end the group, the tests to use it, and a read that waits for a program's line to have a limit, so a line that never comes is a failure with its words and not a hang | 2026-11-22 | `crates/tondo/tests/common`; `crates/tondo/tests/observe.rs` |
| The time a task has been ready in the queue every worker takes from, only if a stamp at each enqueue measures clean | 2026-11-21 | [decisions](decisions.md): "The scheduler's state where people look" |
| What each worker runs, in the text of `tondo blocked` and of a dump: today only in their JSON | 2026-11-21 | The same |
| An example in `tondo explain` for the codes that have none: 17 of 180 have one, from the guide's blocks. The others get a program that gives the code alone, a case of `tests/cases` named beside the code, shown with what the checker says of it when asked, or with its panic's line: 94 codes have such a case today, and the rest get one written, but those that take a project, a command or the test runner, listed with why | 2026-11-21 | `crates/tondo/src/explain.rs` |
| The idle path's call that lets a worker put beside another go: reached only when a held task ends between the monitor's read and its mark | 2026-11-21 | `testing/mutants/explained.txt` when a run lists it |
| The models' steps read against the code's locks: a model whose step is larger than what the code does at once says nothing of the window inside it. `wake.to` has a call's return from C and the giving back of its worker's place as one step, and the monitor's look and its mark as one, where the code takes two each, which is why it never showed the scheduler's checker stopping a case of blocking calls of C on 2026-11-22, a look between those two steps: split them, state that every worker past the first stands in for a marked call, is owed back or is retiring, and add the test that today's order breaks, in `Wake.tla` as well, which the maintainer keeps current with the model in Tondo. Then the other six, each in both forms | 2026-11-22 | `testing/models` |
| The scheduler's one mutant left open: a worker that looks for tasks and finds one elsewhere wakes the next if the shared queue filled meanwhile, which neither a test nor the model shows. The model of waking needs a task woken outside any task, and its fairness made to follow the code | 2026-11-08 | [the refactor](refactor.md), status of phase 0 |
| The heap's checks in the Nightly on musl and macOS: on Linux with glibc alone to begin with. Their first run there, on 2026-11-22, was clean; a few more, then the other two | 2026-11-22 | `.github/workflows/nightly.yml` |
| Tests that hold work on speed, asked by the maintainer on 2026-11-25 after faults reached pull requests with every check green. Of those faults, a test would have named two: the build that went from 2.3 s to 10, which CI showed as three time limits that read like chance, and the view's bit that the reader of core dumps did not know; the layer of TLS is held by the rule against writing one, and the pin's count could not be reached. Decided, in this order. First, with the first pull request's rework, which is judged by them: each case's release build timed against main's, failing past twice the time and a second more, in the gate and in CI, naming the case and both times; one description of each layout that the runtime, the code generator, the reader of core dumps and the debugger read, with a dump of each form of a text; and a working rule, that a change to a memory ordering or to a read without a lock in the runtime comes with its model or its written reason and a run on a real arm64 before its push. Then a model for each protocol kept by atomics that has none, the retiring of a worker put beside another first, then a C thread's call against those that inspect its task, then a `Join`'s end read without its lock. Then, before the weighing by trial is written again, a generator of programs in the Nightly that compares each with the MIR's passes and without them. Then the runtime's unit tests under the sanitizers of addresses and of threads in the Nightly, programs once a task's switch of stack tells the sanitizer; and the seeded, stalled and ordered runs of the cases added to a pull request's CI. The whole rule stays a job of this machine, as GitHub's runners give no counters to trust | 2026-11-25 | The audit of the three pull requests; `crates/tondo-runtime/src/tasks/threads.rs`, `crates/tondo/src/cores.rs` |

## The manual

| What | Since | Detail |
| --- | --- | --- |
| Diagnostics the pages quote that no test holds, nine: those of a project of several modules (chapters 5 and 15, and the three of `tondo check` in a project the daily page builds up), of a provider (chapter 17), and of another system than the one that runs the tests (chapters 15 and 18, and the processor's feature of the performance page). A project's would take a fence that gives a file its path and a listing run in it | 2026-11-21 | [decisions](decisions.md): "A listing is held to what it shows" |
| Listings with times, sizes, dumps or machine code, and those of a project built up by commands, held by nothing: run again by hand after a change to the tool that prints them | 2026-11-21 | The same |

## Waiting for the maintainer

| What | Since |
| --- | --- |
| The views of texts, once their numbers are in under the whole rule: he waits for them, and it is an optimization he has long wanted (2026-11-25). The third pull request holds a form of them | 2026-11-22 |
| Tail calls: not yet, by his word of 2026-11-25. When it comes, whether a call in tail position reuses its frame never, always, or where the code asks, what a trace and the debugger show of the frames that went, and that a call with a `defer` pending is not in tail position | 2026-11-25 |
| The three pull requests of another session closed without merging, each when what replaces it is on main, with a comment saying where each part went and what was dropped; their branches stay until he says | 2026-11-25 |

## Parked

In the README's Status: vectors of 512 bits, profile-guided optimization,
logpoints that leave nothing in Release code, Windows, a library for user
interfaces, an image to debug in containers, the mouse in a terminal, a
newer allocator on macOS, and a first release. And, by the maintainer's
word of 2026-11-22, trying the editor's extension in a real VS Code and
publishing it: not yet.

By the maintainer's decisions of 2026-11-09, in [the speed plan](speed.md)'s
status, and not forgotten: Cranelift's code kept by function (F2), later;
the checker on every core (J), left as its numbers say; and a reader of
TOML's events without the tree, which waits for a program that decodes
TOML often.
