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

1. **What is owed**: what is left of the cost the scheduler's pieces added
   to a spawn and a join, and what the next Nightly's jobs under emulation
   show. First because they say whether what is on main is sound, and
   anything they find is fixed before more is built on it.
2. **`trim`**: the inliner weighing a call by what inlining buys and a
   region for the copy, which the plain form in Tondo, measured again,
   waits for; then the list of what stays behind Go. Second because its rows
   are the furthest behind, and the inliner's weighing is a piece many other
   calls wait for.
3. **What the maintainer said yes to on 2026-11-23**, after the inliner by
   his word: the orphans of a program that is a container's first process,
   a small piece; then `std.quic`'s first piece, measured, which says what
   HTTP/3 and WebTransport cost after it. The archive read on demand comes
   here too, as it waited for the inliner alone.
4. **The chain's rest**: the loop's fast arm after its branch, which is
   also what the chars rows lack, and the reads of the task's attention.
   Then a new table against Go, at the baseline and at x86-64-v3. Here
   because the diagnosis is done and fresh, and the table is what orders
   the steps after it.
5. **What the examples and the rows left pay**, by one round of profiles
   taken again: the library functions, `url.encoded` first, the map copied
   in a loop over it, and programs/sum lines and queries/countBy in the
   same hour, which is the same work with the same tools. The examples'
   are a larger share of real programs than any row left, and the round
   says which rows texts reach.
6. **The views of texts**: their two numbers, which steps 2 and 5 give,
   then the maintainer's decision.
7. **An array of tasks waited for as one wait, then `parallel for` and
   `.parallel()`**: the largest piece, after what it stands on.
8. **The tightening**: texts' counts at direct sites, the header at 16
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
| What the Nightly's jobs under emulation show once they run whole. In the run of 2026-11-22 they built again and then 106 tests failed on the two architectures, those of the debugger, dumps, profiles and the rest, and their binary did not end: a unit test of 2026-11-21 made the tools' directory open to others before any program had made it private, in the fresh container each binary runs in, and every program then refused the tools, as it should. The test is mended; the next run says whether that was all of it. Older there, and each to read when a run shows its words: `vectors_at_the_level_of_avx2` on x86-64 under qemu, whose words no log has; the cases' binary running past its hour and a half there, slow and not hung; `a_library_that_tondo_built_ends_and_unloads` and `cpu_has_answers_from_the_level_or_the_processor`, which failed on 2026-11-21 and not on the 22nd; and observe's `a_cpu_profile_of_a_program_that_waits_says_it_waited` on arm64, red from 2026-11-16 to 21 and hidden since | 2026-11-22 | Nightly runs 45051265708 and 44896932412 |
| The readers of bytes or text from outside that have no search in the Nightly, two lots left of three. The first is in: a GraphQL document, the forms of a request, its cookies and a query's parts, and it found three ways for a document of GraphQL to end a server, mended with it. Left: the images' decoders and their EXIF, certificates and keys in PEM and DER, and CSV; then the decoders of `std.encoding`, a UUID, the parsers of dates and durations, the streams of gzip and zstd, and gRPC's frames; MySQL's packets last, as no public call reaches their reader. Each search runs some minutes before it lands, and what it finds is mended with it | 2026-11-22 | `testing/fuzz` |
| Eight of the Nightly's searches change their input blind: what they try is the runtime's own code, of which the search's coverage sees nothing. They are the ones of JSON, YAML, TOML, MessagePack, CBOR, protobuf, the regex and a request's form in parts; the images' decoders and the inflaters, which have none yet, would be the same. A runtime built for `--fuzz` with the sanitizer's counters over its Rust, read by the search beside its own, gives all of them eyes at once: a piece of days, after the inliner's weighing. Until then each such search says in its own comment that it is blind | 2026-11-22 | `testing/fuzz`; `crates/tondo/build.rs` |
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
| processes/true by name, by path | at Go's time, 0.98× and 0.97×; 474 allocations against 59 and 456 against 34, three times the bytes | The environment copied whole at each start, twice, and built again as the child's: some 300 of the allocations with 55 variables. The child's environment prepared once and made again only when the program changes a variable; nothing passed at all when a start adds nothing, so the system's own is used. And the lookup of a program by its name on the child's `PATH`, in since 2026-11-22: measured again on 2026-11-23 with both sides of the comparison in one environment, it costs a start by name 5.24% more instructions, 175.7k against 166.9k, and 21 allocations, 475 against 454, where 2.8% was written, the comparing tool's extra variable having hidden the rest. It makes three allocations for each directory of `PATH` it tries, two joins and the text for the check: one buffer for all of them takes those back. Its time and cycles, which fell and are why it was taken, measured again in one environment on a quiet machine | 2026-11-22 |
| http/hello, http/json, in bytes | 3.15× and 1.5× Go's bytes a request, 20.4 KB against 6.5 and 22.3 against 14.7, with fewer allocations than Go, 49 against 69 a request in `hello` | The memory profile by function of `hello 1x1000` sees 41 of the 49 allocations and 3.9 of the 20.4 KB: about 8 allocations and 16.5 KB a request, four fifths of the bytes, are in none of its lines, though the benchmark's counters count them. So first which allocations the profile misses, [a hole of the tool](#tools-and-tests), then whose they are. A lead and no more: two blocks of 8 KiB a request would be 16,384 of those bytes, and 8,192 is what hyper first gives a connection's read buffer. Of what the profile does see, a third is the headers' map, 1,312 bytes in 4 allocations, then the client's response, 559, and the server's request, 480. The bench page gives `json`'s number with no reason and `hello`'s not at all. The bound on a response adds 24 of these bytes, 0.1% | 2026-11-23 |
| net/tls echo | 1.03–1.05× | rustls's buffers, an allocation a record each way, which its API without buffers would not make | 2026-11-12, [speed plan](speed.md), H2 |
| ffi from a C thread, texts/toUpper other, tasks/spawn, runtime/mutex 4x1000, programs/count words | under 1.10× | No piece yet. The first and the third do 3.2% and 2.3% more work than at the table, by the clock read at each spawn and join: [owed](#owed-now) | 2026-11-21 |
| The table at x86-64-v3 | of 2026-11-15, before a week of the optimizer's work | Taken again with the next table | 2026-11-14 |
| A new table against Go | | After the chain's two pieces, the fast arm and the attention's reads | 2026-11-21 |

## Speed work decided and not done

| What | Since | Detail |
| --- | --- | --- |
| The bound on a response read whole costs each request of the client 24 bytes, three words kept with it: how much more it may give, the same for the decoder's sink, and the bound the request carries to its response. The sink's is held even by a response with no `Content-Encoding`, which has no decoder. To try, with the next piece that touches the client: the sink's word in the decoder's own state, measured on `http/hello` | 2026-11-23 | `crates/tondo-runtime/src/client.rs`, `Received`; measured by the memory profiles of the rows |
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

## The library, decided and not done

| What | Since | Detail |
| --- | --- | --- |
| The orphans of a program that is a container's first process. A command's own children go to the first process when that command ends, and a Tondo program reaps only what it started, so they stay in state Z for as long as it runs. By the maintainer's word of 2026-11-23 the runtime reaps them by itself when it is pid 1, as .NET does: it looks first and takes only a child that none of its tables holds, and a program that is not the first pays nothing. What no shape can tell from an orphan is the child of a C library reached through a foreign call, whose own wait would then fail: the manual says so. Its case runs as a container's first process; the design before the code | 2026-11-23 | Seen with a case's static build as pid 1 of a plain `alpine` container; the runtime waits for each child by its pid today |
| QUIC, HTTP/3 and WebTransport, beside the HTTP/1, HTTP/2 and WebSockets the library has, in three pieces by the maintainer's word of 2026-11-23. `std.quic` first, a measured piece on the workers' poller with `quinn-proto` over the rustls the runtime has, whose numbers say what the other two cost. Then HTTP/3 in the server and the client, where the choice is between the young `h3` and the protocol's framing written here. Then WebTransport beside `WebSocket`, which follows a draft that still moves. Each with its tests against other implementations and its rows against quic-go, as Go's own library has none ready; the design of each before its code | 2026-11-22 | Read on 2026-11-23: `quinn-proto` 0.11.19, `h3` 0.0.8 and `h3-webtransport` 0.1.2 of May 2025, the draft's seventeenth of November 2026, in Chrome, Firefox and Safari 26.4 |

## Tools and tests

| What | Since | Detail |
| --- | --- | --- |
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

## The manual

| What | Since | Detail |
| --- | --- | --- |
| Diagnostics the pages quote that no test holds, nine: those of a project of several modules (chapters 5 and 15, and the three of `tondo check` in a project the daily page builds up), of a provider (chapter 17), and of another system than the one that runs the tests (chapters 15 and 18, and the processor's feature of the performance page). A project's would take a fence that gives a file its path and a listing run in it | 2026-11-21 | [decisions](decisions.md): "A listing is held to what it shows" |
| Listings with times, sizes, dumps or machine code, and those of a project built up by commands, held by nothing: run again by hand after a change to the tool that prints them | 2026-11-21 | The same |

## Waiting for the maintainer

| What | Since |
| --- | --- |
| The views of texts, once their two numbers are in | 2026-11-22 |

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
