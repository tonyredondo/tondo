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

1. **What is owed**: the archive's readers, which trust the numbers of the
   file they read; what is left of the cost the scheduler's pieces added to
   a spawn and a join; what a native that waits inside gives back when a
   deadline ends its wait; and the next Nightly's jobs under emulation.
   First because they say whether what is on main is sound, and anything
   they find is fixed before more is built on it.
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
| What a native that waits inside gives back when a deadline ends its wait. The one that runs a process and reads what it wrote returned its whole value with the deadline noted as well, where 11.17 says that the operation that waited gives nothing back; found as the deadline's form of the leak, which no longer leaks. And a `fs.readText` of a named pipe under a `within` of 10 ms was still waiting after two seconds, seen once and to confirm. To say which waits a deadline ends, and what each native that waits inside does when it ends one | 2026-11-22 | Spec 11.15 and 11.17; `tests/cases/run/within_given_back.to` |
| Whether the Nightly's jobs under emulation build and pass. In the run of 2026-11-22 they did not build: the build for the other architecture linked for a C library older than the one published, which lacks a function the path of a crash calls. It links for the published one now and its step prints the compiler's words; the next run says whether it builds, and whether observe's `a_cpu_profile_of_a_program_that_waits_says_it_waited`, red there every day from 2026-11-16 to 21, holds under emulation. That run's steps with the seeded stops and with the heap's checks were clean | 2026-11-22 | Nightly run 45017098543 |
| `std.archive` trusts the numbers of the file it reads: a size or a date of a tar in its base-256 form, a size of a PAX record, the 8-byte numbers of ZIP64 and the offsets made from them overflow and panic the program, and a negative size of a PAX record keeps the reader in its loop for ever. Each is to fail as a damaged archive, with a search over the readers' input kept in the Nightly. And a certificate's time whose fields are out of range is taken as another date without a word | 2026-11-22 | `crates/tondo/std/archive.to`, `crypto.to`; found after the template's arithmetic |
| `archive.read` inflates a whole `tar.gz` or `tar.zst`, and every deflated entry of a zip, before the program sees an entry, so a few kilobytes made for it fill the memory. It asks for a bound on what a read may inflate, or for entries read as they are asked for: the maintainer's to see, with the numbers | 2026-11-22 | `crates/tondo/std/archive.to` |
| The readers of bytes or text from outside that have no search in the Nightly. The one of templates found a panic the night it was read; the archive's is being added | 2026-11-22 | `testing/fuzz` |

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
| mcp's `runs_tests_and_debugs` failed once on x86-64 macOS, in a job that took twice its time: the search for the input that fails kept another as its finding, "an input ran longer than --fuzz-timeout, 1s", for a call that compares a number, because the runner held the process for that second. A search on a shared machine meets such a stall, so an input that runs past the limit is to be run again, alone, before it is kept, and one that then passes counted apart in the report; the same while a timeout is reduced | 2026-11-22 | `crates/tondo/tests/mcp.rs`; CI run 44977475458, job 137070490714; testing spec 10.6 |
| The code a benchmark's row reaches that the other Tondo makes differently is named only when its count of instructions changed: a function whose machine code changed with the same count, as the workers' loop with another allocation of its registers, is not on the list, and it was what moved `ffi/waits from a C thread` | 2026-11-22 | `tondo test --bench --compiler` |
| A leak's report that names what leaked: `TONDO_RT_CHECK_LEAKS` gives one number, and the hunt of 2026-11-22 needed a runtime built for it to learn that the object was a join. It is to keep each counted object's kind and the place that made it, and print the ones left at exit, at no cost when the check is off | 2026-11-22 | `crates/tondo-runtime/src` |
| The heap's checks see nothing of what the runtime keeps on lists of its own to use again, as freed joins: to the allocator they are alive. A runtime built with the checks is to give them back to the allocator, so a stale use of one faults | 2026-11-22 | `TONDO_RUNTIME_HEAP_CHECKS` |
| `tondo size --against` moves some hundred bytes between the program's data and the runtime's when only the runtime changed, summing to nothing: it gives data to a symbol, keeps one symbol of those at one address and counts bytes without a name as the runtime's | 2026-11-22 | `tondo size` |
| The time a task has been ready in the queue every worker takes from, only if a stamp at each enqueue measures clean | 2026-11-21 | [decisions](decisions.md): "The scheduler's state where people look" |
| What each worker runs, in the text of `tondo blocked` and of a dump: today only in their JSON | 2026-11-21 | The same |
| An example in `tondo explain` for the codes that have none: 17 of 180 have one, from the guide's blocks. The others get a program that gives the code alone, a case of `tests/cases` named beside the code, shown with what the checker says of it when asked, or with its panic's line: 94 codes have such a case today, and the rest get one written, but those that take a project, a command or the test runner, listed with why | 2026-11-21 | `crates/tondo/src/explain.rs` |
| The idle path's call that lets a worker put beside another go: reached only when a held task ends between the monitor's read and its mark | 2026-11-21 | `testing/mutants/explained.txt` when a run lists it |
| The models' steps read against the code's locks: a model whose step is larger than what the code does at once says nothing of the window inside it. `wake.to` has a call's return from C and the giving back of its worker's place as one step, and the monitor's look and its mark as one, where the code takes two each, which is why it never showed the scheduler's checker stopping a case of blocking calls of C on 2026-11-22, a look between those two steps: split them, state that every worker past the first stands in for a marked call, is owed back or is retiring, and add the test that today's order breaks. Then the other six | 2026-11-22 | `testing/models` |
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
| What bounds an archive that inflates to more than the program can hold, once its numbers are in | 2026-11-22 |
| Whether `Wake.tla` follows `wake.to` when the model's steps are split, or the TLA+ models are left behind | 2026-11-22 |

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
