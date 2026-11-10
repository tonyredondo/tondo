# Testing

`tondo test` runs the tests of a project in parallel worker processes.
`tondo guide runbook-testing` goes through every kind of test on one change,
and says when each is worth it.

```text
src/parse.to           the code
src/parse_test.to      unit tests: they see what the module keeps private
tests/api.to           an integration test: it sees only what is public
```

```text
// src/parse_test.to
import std.testing

test parsesNumbers {
    assert(parse("42") == ok(42))
}

test rejectsText {
    match parse("x") {
        err(_) => {}
        ok(value) => testing.failNow("parsed {value}")
    }
}

test usesAnError {
    let value = parse("7")?          // a test may fail with an error
    assert(value == 7, "got {value}")
}

suite slow {
    let data = load()                // setup, once per worker
    defer cleanup()                  // teardown

    test readsData {
        assert(data.length() > 0)
    }
}
```

- `assert(condition)` and `assert(condition, "message")` fail a test.
- `recover { ... }` tests that code panics, and how:
  `assert(recover { parse(bad) }.isErr())`, or `match` its `err(p)` to read
  `p.code` and `p.data`. It does not stop `testing.skip` or `failNow`.
- `std.testing`: `log`, `tags`, `skip(reason)`, `failNow(message)`,
  `attach(name, mediaType, bytes)`, `snapshot(name, value)` (compares the
  value's JSON with a file; `tondo test --update-snapshots` writes it), and
  `withVirtualTime` for code that sleeps.
- `tondo test --watch` runs the tests again as files change: those the
  change can affect, through the imports.
- `tondo test --coverage` says how much of the project's code the tests
  ran: lines, functions, and the ways of `if`, `match`, `and` and `or`;
  `--report coverage=<path>` writes it as JSON and `--report lcov=<path>`
  as LCOV.
- `--report markdown=<path>` and `--report html=<path>` write what a run
  found for people: failures, skips, benchmarks and coverage; the Markdown
  fits a pull request or a CI summary, the HTML is one file with charts.
- `tondo test --filter text`, `--exact id` (again for more), `--list`, `--jobs n`,
  `--timeout s`, `--retry n`, `--lock-order`, `--order random --seed hex`,
  `--report junit=out.xml`; `--json` gives the report as data, and
  `--test-format events` a line of JSON for each result as it comes.
- `tondo test --affected <rev>` runs only the tests that reach what changed since a
  commit, through what they call.
- `tondo test --bisect <commit> --filter text` finds the first commit after that one
  where a test fails, in a worktree of its own, with the tests as they are here: one
  written today, as `tondo dump test` writes, finds when the bug came.
- A test that calls `testing.bench` in its body is a benchmark (below).
- A test that ends with tasks still running fails, with where each waits and
  where it was spawned: wait for them, cancel them, or close what they wait on.
- `tondo test --schedules 50` runs each test with its tasks in 50 orders, to
  find one that depends on which task runs first; a failure prints the
  command that runs that order again, `--schedules 1 --seed hex`, and the
  one that debugs the test in it, `tondo debug --test <id> --schedule hex`.
- `tondo test --schedules all` tries every order that differs instead of some,
  fewest changes first, and says how many and whether that was all of them, up
  to 20 000 runs for each test, or `all:<n>`. Two orders differ only if two tasks
  touch the same thing the other way around: a lock, an `Atomic`, a channel, a
  collection of `std.sync`, or anything outside the program, so tasks that share
  nothing have one order however long they run. A failure gives `--schedules at:0.2.1`, which repeats it, and
  `tondo debug --test <id> --schedule at:0.2.1`. It suits a small test of what
  tasks share. Memory shared through `unsafe` is not seen: use `--schedules 50`
  for that. In VS Code it is *Test in Every Order*, which offers to debug the
  order that failed; through `tondo mcp`, `test` with those flags gives it as
  `orders.failed_at`, for `debug_start` to take as `schedule`. Nothing waits for the clock
  in these runs: time jumps to the next timer, and that it comes before an
  answer is one more order, so a test with a timeout is tried both ways.
- A test that starts tasks, itself or through what it calls, and passes
  without reaching outside the program, `tondo test` runs again in every order
  of its tasks that differs, up to 200 orders or 2 seconds: a lost update
  shows in a plain run, with the order to repeat it. One that reaches files,
  the network, processes, a database or C runs once, as running it again
  would change them: `--schedules all` tries its orders when it leaves them as
  it found them. `--schedules 1` runs each test in one order.
- A run says what it did not try: those tests, and functions that give the
  next state for an event with no test of them in every state, for which
  `tondo model <function>` writes one.
- `tondo test --record target/recordings`, on Linux, records each test and
  keeps the runs of those that fail: `tondo debug --replay
  target/recordings/<id>` debugs one again, the same, with what it read
  then, as a test that fails once in CI.

## Benchmarks

```text
// src/parse_test.to
import std.json
import std.testing

test parsing {
    for size in [10, 1_000] {
        let text = document(size)
        testing.bench("parse {size}", () { json.parse(text) })?
    }
}
```

- A test that calls `testing.bench(name, closure)` in its own body is a
  benchmark; anywhere else, in a function, a closure or a suite's setup,
  the call is `E2006`. The call fails as the closure does: `?` passes it on.
- `tondo test` runs each closure once, so a benchmark that breaks fails like
  any test.
- `tondo test --bench` runs only the benchmarks, one at a time and
  optimized, as the program runs outside the tests: without the watches of
  lost updates and lock orders that a plain run keeps. Each warms up, takes
  50 samples in `--bench-time` seconds (3 by default) and counts what a run
  allocates, then prints the time a run takes, its spread, its allocations
  and bytes, and on Linux the instructions the processor ran for it:
  `parse 10  1.973 µs/op  ±0.5%  51 allocs/op  1685 B/op  24.1k ins/op`.
  `--test-format json` gives every sample, and the machine.
- What the closure gives is kept, so the work that made it is not optimized
  away; `testing.keep(value)` keeps a value the closure does not give.
- `--save-baseline main` keeps what was measured in
  `target/bench/main.json`, and `--baseline main` compares with it: each
  benchmark says how much it changed, and whether that is `slower`,
  `faster` or the `same` with 95% confidence.
- `--profile cpu` (or `memory`) writes a profile of each test, or with
  `--bench` of what each benchmark measured, to `target/profiles`, which
  `tondo profile show` reads (topic `stdlib`).
- `--exec "perf record -g"` runs the tests' executable under a command, as
  a profiler, `valgrind` or `strace -f`: the command's words, then the
  executable and its arguments. What the command writes goes to the files
  it names, as `perf stat -o stat.txt`; the runner keeps the tests' output.
- `--against main` builds the tests of a git revision and runs each next to
  these on the same machine; with `--max-regression 5`, a benchmark 5%
  slower, or allocating more, fails the run: a gate for CI. With
  `--regression-by instructions` it judges the instructions of a run
  rather than its time, which moves with where the code lies and with
  what else runs on the machine.

## Fuzzing

```text
// src/parse_test.to
import std.json
import std.testing

test parsesAnyDocument {
    testing.fuzz("parse", ["{{}}", "[1, 2]", "\"x\""], (text: String) {
        match json.parse(text) {
            ok(value) => assert(json.parse(json.encode(value)) == ok(value)),
            err(_) => {}
        }
    })
}
```

- A test that calls `testing.fuzz(name, seeds, closure)` in its own body
  tries inputs; anywhere else the call is `E2007`. The input is a value of
  any type with `Encode` and `Decode`, which records, enums and newtypes get
  by themselves: a `String`, an `Array[Byte]`, a `Request { ... }`.
- An input fails when the closure does: an `assert` or a panic ends the
  test, and an error ends the call, which gives it (`?` passes it on).
  Handle in the closure what the code must reject.
- `tondo test` runs the closure with each seed, then with each input that
  failed before, kept as JSON in
  `fuzz/<kind>/<module>/<suites...>/<test>/<name>/<hash>.json`: commit them,
  so that each stays a regression test. A failure says which input, and
  from which file; one that no longer reads as the type is `P2011`.
- `tondo test --fuzz` then searches: it changes inputs knowing their type,
  with values from the code's literals and from what its comparisons
  meet, keeps those that run code of the project or its dependencies no
  input ran before, and stops at Ctrl-C, after `--fuzz-time` seconds, or
  when an input fails, which it reduces while it fails the same way and
  saves to the store. Put the code it tries in the project's sources, not
  in the test file: only theirs is counted. An input that runs longer than
  `--fuzz-timeout` (1 s), holds more than `--fuzz-memory` MiB (2048) or
  crashes fails too. `tondo debug --test <id>` stops the test where it
  fails. Through `tondo mcp`, the `test` tool searches with `fuzz: seconds`. `--jobs` workers share the
  search; what it found is in `target/fuzz/`, or `--fuzz-corpus <dir>`,
  and the next search starts from it.
