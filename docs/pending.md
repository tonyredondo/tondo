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

Last gone through on 2026-11-22, against main at 466659e3.

## The order

What is done next, and after it, with the reason for each place. A step's
detail is in the tables below. The inliner's weighing of step 2 comes
before the chain's rest because the chain's adapters wait for it too.

1. **What is owed**: the bounds on what the library reads whole from
   outside, with the maintainer's numbers; what is left of the cost the
   scheduler's pieces added to a spawn and a join; and the next Nightly's
   jobs under emulation. First because they say whether what is on main is
   sound, and anything they find is fixed before more is built on it. The
   archive read on demand, the larger half of the same answer, comes after
   the inliner's weighing by the maintainer's word.
2. **`trim`**: the inliner weighing a call by what inlining buys and a
   region for the copy, which the plain form in Tondo, measured again,
   waits for; then the list of what stays behind Go. Second because its rows
   are the furthest behind, and the inliner's weighing is a piece many other
   calls wait for.
3. **The chain's rest**: the loop's fast arm after its branch, which is
   also what the chars rows lack, and the reads of the task's attention.
   Then a new table against Go, at the baseline and at x86-64-v3. Third
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
| Whether the Nightly's jobs under emulation build and pass. In the run of 2026-11-22 they did not build: the build for the other architecture linked for a C library older than the one published, which lacks a function the path of a crash calls. It links for the published one now and its step prints the compiler's words; the next run says whether it builds, and whether observe's `a_cpu_profile_of_a_program_that_waits_says_it_waited`, red there every day from 2026-11-16 to 21, holds under emulation. That run's steps with the seeded stops and with the heap's checks were clean | 2026-11-22 | Nightly run 45017098543 |
| The bounds on what the library reads from outside, in the one form the maintainer chose on 2026-11-22: whatever bounds takes `limits:`, a record of its module named `…Limits` whose fields begin with `max`, with a `Default`, and its error says the limit and the code that raises it. To write: `ClientLimits.maxBodyBytes` for a response read by `text()`, `bytes()` or `json()`, 2 GiB; `GzipLimits` and `ZstdLimits.maxOutputBytes`, 128 MiB; `IoLimits.maxLineBytes` for `readLine`, 1 MiB; `GrpcLimits.maxMessageBytes`, 16 MiB, in place of today's constant; `ImageLimits.maxDecodedBytes`, 512 MiB, in place of the crate's; `ArchiveLimits.maxInflatedBytes`, 1 GiB while an archive is read whole; and `maxNestingDepth` in the limits of JSON, protobuf, CBOR, XML, YAML, MessagePack, TOML, GraphQL and a regex, each a constant or a crate's own number today. The HTTP server's `Limits` comes into the form as `ServerLimits`. The depths' defaults wait for what a task's 8 MiB of stack holds, measured for each decoder | 2026-11-22 | [decisions](decisions.md) |
| The readers of bytes or text from outside that have no search in the Nightly, two lots left of three. The first is in: a GraphQL document, the forms of a request, its cookies and a query's parts, and it found three ways for a document of GraphQL to end a server, mended with it. Left: the images' decoders and their EXIF, certificates and keys in PEM and DER, and CSV; then the decoders of `std.encoding`, a UUID, the parsers of dates and durations, the streams of gzip and zstd, and gRPC's frames; MySQL's packets last, as no public call reaches their reader. Each search runs some minutes before it lands, and what it finds is mended with it | 2026-11-22 | `testing/fuzz` |
| Eight of the Nightly's searches change their input blind: what they try is the runtime's own code, of which the search's coverage sees nothing. They are the ones of JSON, YAML, TOML, MessagePack, CBOR, protobuf, the regex and a request's form in parts; the images' decoders and the inflaters, which have none yet, would be the same. A runtime built for `--fuzz` with the sanitizer's counters over its Rust, read by the search beside its own, gives all of them eyes at once: a piece of days, after the bounds on what the library reads and the inliner's weighing. Until then each such search says in its own comment that it is blind | 2026-11-22 | `testing/fuzz`; `crates/tondo/build.rs` |
| The reader of a request's form in parts written in Tondo, on its own merits: measured against the runtime's at a megabyte first | 2026-11-22 | `httpNative.requestForm` |

## Rows behind Go

The table of 2026-11-21 has 60 of 75 rows at or under Go's time.

| Row | Behind by | What is left | Since |
| --- | --- | --- | --- |
| texts/trim, texts/trim read | 3.4×, 3.25× | The inliner weighing a call by what inlining buys, and a region for the copy of a function over its budget that returns its parameter or what it made; then what still stays behind Go, by function and with its machine code, as a list for the optimizer. `trim` in Tondo, measured again on 2026-11-22, takes 558 instructions against the native's 325, with a heap allocation, for those two causes and no other; its form waits for them | 2026-11-20, [decisions](decisions.md): "Trim stays a native until a call is weighed by what inlining buys" |
| queries/numbers iter map filter sum | 275.0k instructions against 142.8k, with no count for a parameter's parts (285.1k without) | The loop's fast arm placed after its branch; the two reads of the task's attention an element, once it is said which check they are; the address of that flag held in a register through the loop; and where the total lives, to read again in the machine code | 2026-11-21 |
| queries/iter filter sumBy, queries/countBy | 1.71×, 1.18× | The same loop's rest; `countBy` by its own profile | 2026-11-21 |
| texts/chars ascii, texts/chars other | 2.24×, 1.54× | Two taken jumps a byte against Go's one, which the fast arm after its branch lifts; and the loop's count checked for overflow each turn, which is Tondo's to keep unless it is proven | 2026-11-21 |
| programs/sum lines | 2.40×, 5 allocations against 2 | Its profile first: the reader and `readLine`'s path. Its lines are short texts, which nothing copies | 2026-11-21 |
| net/tls echo | 1.03–1.05× | rustls's buffers, an allocation a record each way, which its API without buffers would not make | 2026-11-12, [speed plan](speed.md), H2 |
| ffi from a C thread, texts/toUpper other, tasks/spawn, runtime/mutex 4x1000, programs/count words | under 1.10× | No piece yet. The first and the third do 3.2% and 2.3% more work than at the table, by the clock read at each spawn and join: [owed](#owed-now) | 2026-11-21 |
| The table at x86-64-v3 | of 2026-11-15, before a week of the optimizer's work | Taken again with the next table | 2026-11-14 |
| A new table against Go | | After the chain's two pieces, the fast arm and the attention's reads | 2026-11-21 |

## Speed work decided and not done

| What | Since | Detail |
| --- | --- | --- |
| Texts that share another's bytes where a result does not leave the turn (views): two numbers first, what `trim` in Tondo leaves on its rows and how many texts of over seven bytes made by `slice`, `trim` and the pieces of `split` die in their turn in the examples; then the maintainer decides. The loans measured on 2026-11-19 paid for a word read by every text; this form has not been built | 2026-11-22 | [decisions](decisions.md): "Parts of texts lent were built, measured and set aside" |
| The library functions the examples' profiles named: `url.encoded` (39.9% of linkcheck's instructions), `ffi.fromWide` and `ffi.wideUnits` (7.4% and 6.5% of profiler's), `splitOnce`, `fs.exists`, sql's bound parameters, the regex iterator, and a search for one byte paying the set-up of a search for many | 2026-11-19 | Profiles of the examples at f365a7da; to measure again before each |
| A map copied whole at each write inside a loop over it: 54% of chat's allocations. The loop goes over a snapshot that holds the map, and each write through the loan copies it; the first shape, a parameter a function takes, is done | 2026-11-19 | Profiles of the examples at f365a7da; the two ways to lift it were to be weighed and never were |
| A part stored into a value that leaves its function, copied out where it leaves (logstat) | 2026-11-19 | With the views |
| Texts' counts at direct sites, first of the tightening; a text's header at 16 bytes | 2026-11-19 | [speed plan](speed.md), Phase K |
| The inliner weighing a call by what inlining buys | 2026-11-20 | Reopens `trim` |
| An archive read on demand. Today `archive.open` reads the whole file, inflates every entry of a zip and the whole stream of a `tar.gz` or `tar.zst`, and holds twice what it inflates: 2 GiB for a megabyte made for it. It is to take the index alone, a zip's and a plain tar's by reads at an offset and a compressed tar's by one pass that keeps the headers; inflate an entry when it is read, up to 128 MiB when read whole; write each entry as it inflates it in `extract`, up to 8 GiB and a million entries; and give an entry as a stream with `Archive.reader(name)`. With few allocations, as every reading on demand: an entry's stream fills the caller's buffer, and its cases and a row against Go count them | 2026-11-22 | After the inliner's weighing; `crates/tondo/std/archive.to` |
| A process's `output()` and `wait()` wholly on the poller, its pipes and the child's end, which takes out the reader that outlives its call and what keeps it safe | 2026-11-22 | `crates/tondo-runtime/src/process.rs` |
| An array of tasks waited for as one wait | 2026-11-20 | |
| `parallel for` and `.parallel()` in a chain, after the three above | 2026-11-20 | README, Status |
| Phase K: regions tightened, the alignment of loops' heads, and the rows whose time moves by a fifth with where their code lies | 2026-11-18 | [speed plan](speed.md), Phase K |
| gRPC's client on the workers' poller; what still uses tokio: hyper's timer, the server's timeouts, the WebSocket client | 2026-11-10 | [speed plan](speed.md), H2, steps 10 and 11 |
| One debugger session for `tondo dap` and `tondo debug`; text conversions on the runtime's hot paths; YAML's scanner | 2026-11-09 | [speed plan](speed.md), I1, I2, I3 |

## Tools and tests

| What | Since | Detail |
| --- | --- | --- |
| observe's `debugger_goes_back_in_a_replay` fails now and then under load. A copy says its probe since 2026-11-21: the line it prints at its next failure is the lead | 2026-11-16 | `crates/tondo/tests/observe.rs` |
| Which pipe had closed when SIGPIPE ended `tondo dap` on x86-64 macOS, in four of the seven runs of main up to 2026-11-22 (observe's `every_debugged_program_s_end_reaches_the_editor`: "the adapter had exited with signal: 13 (SIGPIPE)", 66 ms into a round). The commands that serve a client or a child for long now ignore that signal and take the write's error, so the adapter no longer dies of it; but its socket to the program cannot raise the signal, so the write was to a pipe that is not yet named. The test's report says the write's error at its next failure, if that was not the end of it | 2026-11-20 | `crates/tondo/tests/observe.rs`; CI run 44991205995, job 137112438117 |
| The code a benchmark's row reaches that the other Tondo makes differently is named only when its count of instructions changed: a function whose machine code changed with the same count, as the workers' loop with another allocation of its registers, is not on the list, and it was what moved `ffi/waits from a C thread` | 2026-11-22 | `tondo test --bench --compiler` |
| A leak's report that names what leaked: `TONDO_RT_CHECK_LEAKS` gives one number, and the hunt of 2026-11-22 needed a runtime built for it to learn that the object was a join. It is to keep each counted object's kind and the place that made it, and print the ones left at exit, at no cost when the check is off | 2026-11-22 | `crates/tondo-runtime/src` |
| The heap's checks see nothing of what the runtime keeps on lists of its own to use again, as freed joins: to the allocator they are alive. A runtime built with the checks is to give them back to the allocator, so a stale use of one faults | 2026-11-22 | `TONDO_RUNTIME_HEAP_CHECKS` |
| `tondo size --against` moves some hundred bytes between the program's data and the runtime's when only the runtime changed, summing to nothing: it gives data to a symbol, keeps one symbol of those at one address and counts bytes without a name as the runtime's | 2026-11-22 | `tondo size` |
| A function of Tondo that a thread of C calls runs on that thread's own stack, with no switch, for speed: half a megabyte on macOS, where XML's or YAML's decode at 128 levels takes more. So the bounds on nesting hold in a task, a handler and `main`, and not there. To measure, on the rows of calls from C, what a switch to a stack of a task's size costs a call; the manual says meanwhile to give deep work a task | 2026-11-22 | `crates/tondo-runtime/src/tasks/foreign.rs`, `tondo_rt_callback` |
| A program started by its name is started by a copy of the whole process, where one given by its path is started without one: the runtime sets the child's environment, its PATH among it, and the library under it then cannot look the name up for the child and falls back to the copy. On macOS the copy costs with the address space set apart: 3 to 7 ms plain, and 238 to 247 ms under the regions' checks, as the cases run, against 1 ms for a path either way. The runtime is to look the name up on the child's PATH itself, as the system does, and start every program by its path: a millisecond for all, in a release build too, and the macOS jobs lose a quarter of a second for each process a case starts | 2026-11-22 | `crates/tondo-runtime/src/process.rs`; measured on arm64 macOS |
| `tests/cases/run/profile_tasks_sleeping.to` showed two tasks nowhere in a thousand snapshots, once, in the musl job of 2026-11-22, and none in 720 runs of the same build under load here. Its code did not change that day. A task that a worker took off the queue and has not yet run shows with no stack, and the snapshot asks again a thousand times, 100 µs apart, and then gives what it has: a worker the system leaves unrun longer than that leaves the task nowhere. A stop that holds a worker there past the snapshot's tries would make it show every time; then a bound that waits for the worker's own step and not for a count of tries | 2026-11-22 | `crates/tondo-runtime/src/inspect.rs`, `snapshot`; CI run 45057663140, job 137316383127 |
| The cases' own test `a_case_that_hangs_says_why` timed out once with every test of the workspace running beside it, in the musl stage run on a developer's machine: it ends a program that sleeps after two seconds and asks for its dump, and the dump did not come in time. Alone it passes. Its two seconds are a margin: the dump is to be waited for by its own word, the dump's file or the answer of the diagnostics, not by a time | 2026-11-22 | `crates/tondo/tests/cases.rs` |
| The time a task has been ready in the queue every worker takes from, only if a stamp at each enqueue measures clean | 2026-11-21 | [decisions](decisions.md): "The scheduler's state where people look" |
| What each worker runs, in the text of `tondo blocked` and of a dump: today only in their JSON | 2026-11-21 | The same |
| An example in `tondo explain` for the codes that have none: 17 of 180 have one, from the guide's blocks. The others get a program that gives the code alone, a case of `tests/cases` named beside the code, shown with what the checker says of it when asked, or with its panic's line: 94 codes have such a case today, and the rest get one written, but those that take a project, a command or the test runner, listed with why | 2026-11-21 | `crates/tondo/src/explain.rs` |
| The idle path's call that lets a worker put beside another go: reached only when a held task ends between the monitor's read and its mark | 2026-11-21 | `testing/mutants/explained.txt` when a run lists it |
| The models' steps read against the code's locks: a model whose step is larger than what the code does at once says nothing of the window inside it. `wake.to` has a call's return from C and the giving back of its worker's place as one step, and the monitor's look and its mark as one, where the code takes two each, which is why it never showed the scheduler's checker stopping a case of blocking calls of C on 2026-11-22, a look between those two steps: split them, state that every worker past the first stands in for a marked call, is owed back or is retiring, and add the test that today's order breaks, in `Wake.tla` as well, which the maintainer keeps current with the model in Tondo. Then the other six, each in both forms | 2026-11-22 | `testing/models` |
| The scheduler's one mutant left open: a worker that looks for tasks and finds one elsewhere wakes the next if the shared queue filled meanwhile, which neither a test nor the model shows. The model of waking needs a task woken outside any task, and its fairness made to follow the code | 2026-11-08 | [the refactor](refactor.md), status of phase 0 |
| The heap's checks in the Nightly on musl and macOS: on Linux with glibc alone to begin with. Their first run there, on 2026-11-22, was clean; a few more, then the other two | 2026-11-22 | `.github/workflows/nightly.yml` |

## The manual

| What | Since | Detail |
| --- | --- | --- |
| Diagnostics the pages quote that no test holds, nine: those of a project of several modules (chapters 5 and 15, and the three of `tondo check` in a project the daily page builds up), of a provider (chapter 17), and of another system than the one that runs the tests (chapters 15 and 18, and the processor's feature of the performance page). A project's would take a fence that gives a file its path and a listing run in it | 2026-11-21 | [decisions](decisions.md): "A listing is held to what it shows" |
| Listings with times, sizes, dumps or machine code, and those of a project built up by commands, held by nothing: run again by hand after a change to the tool that prints them | 2026-11-21 | The same |

## Waiting for the maintainer

| What | Since |
| --- | --- |
| The views of texts, once their two numbers are in | 2026-11-22 |
| QUIC, HTTP/3 and WebTransport, beside the HTTP/1, HTTP/2 and WebSockets the library has: asked for by the maintainer on 2026-11-22, to be weighed first. Owed to him: how hard each is on the runtime's own poller and its rustls, with the choices for the protocol's code, what the API would be beside `WebSocket`, how it is tested against browsers and other servers, and its rows against Go's | 2026-11-22 |

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
