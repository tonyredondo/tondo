# Testing

[The chapter on testing](../language/16-testing.md) is about writing tests:
`test`, `assert`, suites, snapshots, virtual time, benchmarks, fuzz tests
and models. This one is about the command that runs them: how it chooses,
what it prints, and what it does that a test runner usually does not. The
runs shown are of the [examples](../../examples/README.md), with what they
printed.

- [A run](#a-run)
- [A failure](#a-failure)
- [Choosing what runs](#choosing-what-runs)
- [Tests that start tasks](#tests-that-start-tasks)
- [Models](#models)
- [Coverage](#coverage)
- [Benchmarks](#benchmarks)
- [Fuzzing](#fuzzing)
- [Reports, for people and for CI](#reports-for-people-and-for-ci)
- [Finding the commit](#finding-the-commit)
- [Recording a run](#recording-a-run)

## A run

```text
$ tondo test examples/kvstore
test kvstore::integration::server::answersOverTcp ... ok
test kvstore::integration::server::connectionsShareTheStore ... ok
test kvstore::integration::server::noIncrementIsLost ... ok
test kvstore::unit::protocol::anyLineIsACommandOrAnError ... ok
test kvstore::unit::protocol::readsEachCommand ... ok
test kvstore::unit::protocol::repliesAreOneLineEach ... ok
test kvstore::unit::protocol::saysWhatIsWrongWithALine ... ok
test kvstore::unit::store::counts ... ok
test kvstore::unit::store::deletes ... ok
test kvstore::unit::store::listsKeysInTheOrderTheyCame ... ok
test kvstore::unit::store::setsAndGets ... ok
test kvstore::unit::store::theCountOfNumbersIsAlwaysRight ... ok
ok. 12 passed; 0 failed; 0 skipped; 0 blocked; 0.04s
note: 3 tests that start tasks and reached outside the program ran in one order: `--schedules all` tries the others, if each leaves the outside as it found it
```

- A test's id is its package, `unit` or `integration`, its module and its
  name. Unit tests are in `name_test.to` files beside the code and see the
  module's private names; integration tests are in `tests/` and see only
  what is `pub`.
- Tests run in parallel worker processes (`--jobs <n>`), and are reported
  in the same order every run, by id. `--order random --seed <hex>`
  shuffles them, to find a test that depends on another.
- The status is 0 when everything passed and 1 otherwise. A selection that
  matches no test is an error unless the run says `--allow-empty`, and
  `--deny-skips` fails a run that skipped a test.
- The last line, the `note`, says what the run did not try. More on it
  below.

## A failure

```text
$ tondo test
test words::unit::words::countingAPage ... ok
test words::unit::words::countsEachWord ... ok
test words::unit::words::ignoresCaseAndPunctuation ... FAILED

--- words::unit::words::ignoresCaseAndPunctuation
panic[P0007]: assertion-failed: count("To be, or not to be!") == ["to": 2, "be": 2, "or": 1]
  left:  [to: 2, be: 2, or: 1, not: 1]
  right: [to: 2, be: 2, or: 1]
 --> src/main_test.to:8:5
stack:
  test ignoresCaseAndPunctuation  src/main_test.to:8:5

FAILED. 2 passed; 1 failed; 0 skipped; 0 blocked; 0.03s
```

The assertion is one `assert(a == b)`. The runner prints the expression,
both sides as values, the place and the stack. There is no `assertEquals`
with its arguments in an order to remember, and no message to write for the
common case; `assert(condition, "why")` adds one when the values do not
tell the story.

- What a test printed is shown when it fails, and with `--show-output`
  also when it passes.
- `--timeout <seconds>` fails a test that runs longer.
- A test that ends with tasks still running fails, with where each waits
  and where it was spawned.

`--retry <n>` runs a failed test again, in a fresh worker. One that then
passes is not a pass: it is flaky, it is reported with every attempt, and
it still fails the run unless the run says `--allow-flaky`:

```text
$ tondo test --retry 1
--- flaky::unit::flaky::passesTheSecondTime (flaky: it passed after failing)
attempt 1: panic[P0007]: assertion-failed: seen
 --> src/main_test.to:8:5
stack:
  test passesTheSecondTime  src/main_test.to:8:5
stdout:
looking for the marker
attempt 2, retry round 1: passed
stdout:
looking for the marker

FAILED. 1 passed; 1 flaky; 0 failed; 0 skipped; 0 blocked; 0.01s
```

## Choosing what runs

```text
$ tondo test --list
kvstore::integration::server::answersOverTcp
kvstore::integration::server::connectionsShareTheStore
kvstore::integration::server::noIncrementIsLost
kvstore::unit::protocol::anyLineIsACommandOrAnError
...
```

| | |
| --- | --- |
| `--filter <text>` | the tests whose id holds the text |
| `--glob <pattern>` | by a pattern, as `*::unit::store::*` |
| `--exact <id>` | one test, by its whole id; given again for more |
| `--affected <rev>` | only the tests that reach what changed since a commit, through what they call |
| `--shard <i/n>` | one part of `n`, for CI jobs that split a suite |
| `--watch` | runs again as files are saved: the tests the change can affect |
| `--models` | only the tests that check a model |
| `--bench`, `--fuzz` | the benchmarks, or the fuzz tests, as below |

`--affected` and `--watch` use the same knowledge: the compiler knows which
functions each test reaches, so a change to one module runs the tests that
could notice, and no others.

## Tests that start tasks

A bug between tasks shows in some orders of them and not in others, so a
test that passes once proves little. `tondo test` does not run such a test
once. When a test starts tasks and passes, it is run again in every order
of its tasks that differs, up to 200 orders or 2 seconds, and two orders
differ only when two tasks touch the same thing the other way around: a
lock, an `Atomic`, a channel, a shared collection.
[The toolchain](../toolchain.md#every-order-of-a-tests-tasks) shows the
lost update this finds in a plain run.

Time is part of the order. In these runs nothing waits for the clock: it
jumps to the next timer, and that a timer comes before an answer is one
more order to try. This test of the [`warehouse`](../../examples/warehouse)
example waited for a message with a timeout, to be safe:

```text
match changes.receive(timeout: time.seconds(5)) {
    some(change) => ...
    none => "nothing"
}
```

It passed every time it was run by hand, and the runner found what was
wrong with it:

```text
warehouse::unit::shelves::aWatcherIsSentWhatThereIsAndThenWhatChanges: failed in order 3
again in that order: tondo test --exact warehouse::unit::shelves::aWatcherIsSentWhatThereIsAndThenWhatChanges --schedules at:0*3.1.0*6
debug it in that order: tondo debug --test warehouse::unit::shelves::aWatcherIsSentWhatThereIsAndThenWhatChanges --schedule at:0*3.1.0*6

--- warehouse::unit::shelves::aWatcherIsSentWhatThereIsAndThenWhatChanges
panic[P0007]: assertion-failed: next(changes) == "TEA-001 5 0"
  left:  "nothing"
  right: "TEA-001 5 0"
```

Nothing says the task that sends runs within five seconds, and on a loaded
machine one day it would not. The failure comes with two commands: one
that runs that order again, and one that opens the debugger in it.

| | |
| --- | --- |
| a plain run | every order that differs, up to 200 orders or 2 seconds a test |
| `--schedules all` | all of them, fewest changes first, up to 20 000 runs a test |
| `--schedules all:<n>` | up to `n` runs |
| `--schedules <n>` | `n` orders at random, which also covers memory shared through `unsafe` |
| `--schedules 1` | one order: each test once |
| `--schedules at:<choices>` | the order a failure named |
| `--lock-order` | fails a test whose tasks take two locks in both orders: a deadlock that has not happened yet |

```text
$ tondo test --exact warehouse::unit::shelves::ordersForTheLastUnitsCannotAllGetThem --schedules all
warehouse::unit::shelves::ordersForTheLastUnitsCannotAllGetThem: 10074 orders, not all of them: `--schedules all:<n>` tries more
test warehouse::unit::shelves::ordersForTheLastUnitsCannotAllGetThem ... ok
```

A test that reaches outside the program, to files, the network, a database
or C, runs in one order in a plain run, since running it again would
change what is outside. That is the `note` at the end of the first run on
this page. `--schedules all` tries the orders of such a test too, when the
test leaves the outside as it found it.

## Models

A test that checks a state machine with `std.model` explores every state
the machine can reach, not the few a test would think of. `--models` runs
those alone:

```text
$ tondo test examples/kvstore --models
test kvstore::unit::store::theCountOfNumbersIsAlwaysRight ... ok
ok. 1 passed; 0 failed; 0 skipped; 0 blocked; 0.01s
```

`tondo model <function>` writes such a test for a function of the program
that gives the next state for an event, `fn(state: S, event: E): S`, with
no model to write, and a run of `tondo test` names the functions of that
shape that have none. [Testing](../language/16-testing.md) has the models
themselves, and [`std.model`](../std/model.md) every part of them.

## Coverage

```text
$ tondo test examples/kvstore --coverage
...
coverage:
  src/main.to               lines 0.0% (0/4)        functions 0.0% (0/2)      branches 0.0% (0/2)
  src/protocol/protocol.to  lines 94.7% (36/38)     functions 100.0% (4/4)    branches 93.8% (30/32)
  src/server/server.to      lines 95.8% (23/24)     functions 100.0% (11/11)  branches 77.8% (7/9)
  src/store/store.to        lines 100.0% (31/31)    functions 100.0% (4/4)    branches 100.0% (20/20)
  total                     lines 92.8% (90/97)     functions 90.5% (19/21)   branches 90.5% (57/63)
```

Lines, functions, and branches: each way of an `if`, a `match`, an `and`
and an `or`, and the two of a `??` and of a `?.`, whose value was there or
was not. The counts are exact, since the compiler places the counters
where a sampler would guess. `--report lcov=<path>` writes LCOV for the
tools that read it, `--report coverage=<path>` JSON, and the editor shows
the lines in the margin.

## Benchmarks

A test that calls `testing.bench` in its body is a benchmark, and `--bench`
measures them:

```text
$ tondo test --bench
test words::unit::words::countingAPage ... ok
  count     696.2 µs/op  ±34.2%  9622 allocs/op  527160 B/op  6.67M ins/op
```

Time for each operation with its spread, allocations and bytes, and on
Linux the instructions the processor ran. The spread here is wide because
the machine was busy with other work, which is the usual state of a
laptop and of CI. The instructions did not move, and that is what makes
them the measure to judge a change by:

```text
$ tondo test --bench --save-baseline before
$ tondo test --bench --baseline before --max-regression 10
test words::unit::words::countingAPage ... ok
  count     447.7 µs/op  ±1.7%  9622 allocs/op  527160 B/op  6.67M ins/op  -42.3% faster, +0.0% ins, cycles -27.5%, l1d misses -46.6%
```

Nothing changed between those two runs but the machine. By time, the
second is 42% faster; by instructions, it is the same program.

| | |
| --- | --- |
| `--save-baseline <name>` | keeps this run's numbers |
| `--baseline <name>` | compares with them |
| `--against <rev>` | builds and measures a git revision, and compares with it |
| `--max-regression <percent>` | fails the run when a benchmark got worse by more |
| `--regression-by instructions` | judges by instructions instead of time |
| `--bench-time <seconds>` | how long to measure each |
| `--layouts <n>` | measures in `n` orders of the code's functions, and gives the range: how much of a difference is only where the code lies |
| `--profile cpu`, `--profile memory` | writes a profile of the benchmark, for `tondo profile show` |
| `--exec <command>` | runs the tests' executable under a command, as `perf stat` |

`tondo test --bench --against main --max-regression 5 --regression-by
instructions` is a CI job that fails a pull request for making a benchmark
do more work, and is not fooled by a noisy runner.

## Fuzzing

A test that calls `testing.fuzz` states a property of any input. A plain
run tries it on its seeds. `--fuzz` searches for an input that breaks it,
guided by the code each input reaches:

```text
$ tondo test examples/textkit --library --fuzz --fuzz-time 15
fuzzing 2 calls on 2 workers for 0:15
  aSlugOfASlugIsItself › slug  0:01  68k inputs  68k/s  corpus 39  coverage 19%  new 0:00 ago
  distanceIsTheSameBothWays › distance  0:01  24k inputs  24k/s  corpus 30  coverage 32%  new 0:00 ago
  distanceIsTheSameBothWays › distance  0:11  208k inputs  18k/s  corpus 33  coverage 32%  new 0:01 ago
  aSlugOfASlugIsItself › slug  0:11  663k inputs  60k/s  corpus 40  coverage 19%  new 0:09 ago

test textkit::unit::text::aSlugOfASlugIsItself ... ok
  slug  986k inputs in 0:14, corpus 40, coverage 19%
test textkit::unit::text::distanceIsTheSameBothWays ... ok
  distance  281k inputs in 0:14, corpus 33, coverage 32%
ok. 2 passed; 0 failed; 0 skipped; 0 blocked; 15.02s
```

The inputs are of the test's own type: a `String`, a record, a tuple of
two strings. When one fails, the runner reduces it while it still fails
the same way, saves it so that every later run tries it as a regression
test, and prints it.

| | |
| --- | --- |
| `--fuzz-time <seconds>` | how long to search |
| `--fuzz-timeout <seconds>` | an input that takes longer is a failure; one second by default |
| `--fuzz-memory <MiB>` | and so is one that holds more memory; 2048 by default |
| `--fuzz-corpus <dir>` | where the inputs worth keeping are kept, for the next search to start from; `target/fuzz/` by default |

## Reports, for people and for CI

| | |
| --- | --- |
| `--report junit=<path>` | JUnit XML, which every CI reads |
| `--report markdown=<path>` | failures, skips, benchmarks and coverage, for a pull request or a job summary |
| `--report html=<path>` | the same as one file, with charts |
| `--report json=<path>`, `--json` | the whole report as data |
| `--test-format events` | a line of JSON for each result as it comes |
| `--artifacts <dir>` | what tests attached with `testing.attach` |
| `--codeowners auto` | names the owners of each test in the report, from a CODEOWNERS file |

Several `--report` options can be given in one run. The JSON report says
everything about the run that produced it, so that a result can be
reproduced: the selection, the order and its seed, the retries, the state
of the snapshots.

```text
$ tondo test --filter ignores --test-format json
{"format":"tondo-test-report-0.1/1","edition":"0.1","target":{"name":"x86_64-linux","profile":"hosted"},"compiled":true,"selection":{"kind":"filter","value":"ignores"}, ...
```

## Finding the commit

`tondo test --bisect <commit> --filter <text>` finds the first commit after
that one where a test fails. It builds each candidate in a worktree of its
own, with the tests as they are now, so a test written today finds when an
old bug came in.

With `--bench --max-regression 5 --regression-by instructions`, it finds
the commit where a benchmark started to do more work.

## Recording a run

On Linux, `tondo test --record <dir>` records each test as it runs, and
keeps the recordings of those that fail. A test that fails once in a
hundred runs in CI is then a file: `tondo debug --replay <dir>/<id>` runs
it again, the same, with what it read and when, and steps through it
backwards. [Debugging](debugging.md) goes on from there.

In a job of CI, two options and one step make a failure something to open
afterwards: the recordings are uploaded, and the job's summary says the
command for each. In GitHub Actions:

```yaml
- run: tondo test --record recorded --report markdown=$GITHUB_STEP_SUMMARY
- if: failure()
  uses: actions/upload-artifact@v4
  with:
    name: recorded
    path: recorded
```

The summary then has each failure with its report and how to run it again:

```text
### `job::unit::job::halves`

panic[P0007]: assertion-failed: half(7) == 4
  left:  3
  right: 4
 --> src/half_test.to:2:5
stack:
  test halves  src/half_test.to:2:5
recorded, to debug the same again:
  tondo debug --replay recorded/job.unit.job.halves
```

A program of the job that fails outside a test leaves a dump instead:
`TONDO_DUMP=<dir>` in the job's environment, and that directory uploaded
the same way ([A running program](observing.md#in-a-container)).

Next: [Debugging](debugging.md).
