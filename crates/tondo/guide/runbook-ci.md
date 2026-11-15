# Runbook: a test fails in CI and not here

The job's machine is not this one: its environment, its files, its clock and
the order its tasks take are its own. Guessing which of them it was costs a
push for each guess. Set the job up so that a failure leaves what it takes to
open it here, then open it.

```sh
tondo init
```

```tondo file=src/main.to
import std.env

/// A price as the shop shows it: `12.50 EUR`.
fn shown(cents: Int, currency: String): String {
    "{cents / 100}.{cents % 100:02} {currency}"
}

/// The currency of the shop this runs for.
fn shopCurrency(): String {
    env.get("SHOP_CURRENCY") ?? "EUR"
}

fn main() {}
```

```file src/main_test.to
test showsAPrice {
    let price = shown(1250, shopCurrency())
    assert(price == "12.50 EUR")
}
```

Here it passes.

```mcp
test {}
=> showsAPrice ... ok
```

## What the job runs

Two options make a failure something to open afterwards. On Linux, `--record`
records each test and keeps the recordings of those that fail; `--report
markdown=<file>` writes the failures for the job's summary, each with the
command that replays it. The job keeps the directory as an artifact. This is
the job of this runbook, whose pipeline sets a variable this machine has not:

```sh linux
SHOP_CURRENCY=USD tondo test --record recorded --report markdown=summary.md > job.log 2>&1
```

In GitHub Actions the report goes to `$GITHUB_STEP_SUMMARY`, and a step with
`if: failure()` uploads `recorded`. The summary then says:

```text
### `runbook_ci::unit::runbook_ci::showsAPrice`

panic[P0007]: assertion-failed: price == "12.50 EUR"
  left:  "12.50 USD"
  right: "12.50 EUR"
 --> src/main_test.to:3:5
recorded, to debug the same again:
  tondo debug --replay recorded/runbook_ci.unit.runbook_ci.showsAPrice
```

## Opening the failure here

With the artifact downloaded, `debug_start` with `replay` runs that test again
as it ran there, with what it read then: this machine's environment plays no
part. `why` goes back to the line that gave a binding its value, and says
where each thing that line read came from.

```mcp linux
debug_start {"target": "recorded/runbook_ci.unit.runbook_ci.showsAPrice", "replay": true}
debug {"session": "<session>", "command": "break src/main_test.to:3"}
debug {"session": "<session>", "command": "continue", "wait": 30}
=> src/main_test.to:3
debug {"session": "<session>", "command": "why price"}
=> `price` became \"12.50 USD\" on this line
=> src/main.to:10 in shopCurrency
debug_stop {"session": "<session>"}
```

The currency came from line 10, which reads the environment: the job has a
`SHOP_CURRENCY` and this machine none. The code is right and the test is not:
it must say which shop it tests.

```edit src/main_test.to
-    let price = shown(1250, shopCurrency())
+    let price = shown(1250, "EUR")
```

```mcp
test {}
=> showsAPrice ... ok
```

## A program of the job that crashed

A program that panics outside a test leaves a dump, and a job's disk is gone
when it ends. `TONDO_DUMP=stderr` writes the dump into the error output, so it
is in the job's log, and `dump_read` reads it out of the log: the whole log is
its `file`.

```tondo file=report.to
import std.console

fn share(part: Int, parts: Array[Int]): Int {
    part * 100 / parts.sum()
}

fn main() {
    let sold: Array[Int] = []
    console.println("{share(0, sold)}")
}
```

```sh
tondo build report.to --output report
TONDO_DUMP=stderr ./report > report.log 2>&1
```

```mcp
dump_read {"file": "report.log", "view": "why"}
=> panic[P0003]
dump_read {"file": "report.log", "view": "locals"}
=> report.to:4 in share
```

`locals` gives what the frame that failed held: every binding in a build made
with `debug`, and in a release build those its code still held there.
`TONDO_DUMP=<dir>`, with that directory as an artifact, keeps the dumps as
files instead. From a dump: `runbook-crash`, and `runbook-regression` for its
test.

## What else differs there

- **The order of the tasks.** `test` runs a test whose tasks stay in the
  program in every order that differs; for the others, `schedules`. A failure
  names its order, to run again: `runbook-flaky`.
- **The order of the tests.** `--order random --seed <hex>` shuffles them, the
  same way for the same seed: a test that needs another to run first fails
  there. `--jobs 1` runs them one at a time.
- **Less processor.** A runner with one or two cores brings out the waits a
  fast machine hides: `taskset -c 0 tondo test` here, on Linux.
- **The build.** `static`, `target` and `cpu` build as the job does; a
  recording or a dump says which build it is of, and `source` gives its files
  as they were.

## What a job's `tondo test` takes

| | |
| --- | --- |
| `--report junit=<path>` | JUnit XML, which every CI reads; `markdown=` and `json=` too, several at once |
| `--shard <i/n>` | one part of `n`, for jobs that split a suite |
| `--retry <n>` | runs a failed test again: one that then passes is reported as flaky, and still fails the run unless it says `--allow-flaky` |
| `--deny-skips` | fails a run that skipped a test |
| `--allow-empty` | a selection that matches no test is an error without it |
| `--timeout <seconds>` | fails a test that runs longer |
| `--show-output` | what the tests that pass printed, too |
| `--artifacts <dir>` | what the tests attached with `testing.attach` |
| `--codeowners auto` | the owners of each test in the report, from a CODEOWNERS file |

The tool `test` takes them in `flags`, as `["--shard", "1/4"]`. For a failure
that came once, outside a test: `runbook-once`.
