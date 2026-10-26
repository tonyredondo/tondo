# Testing

`tondo test` runs the tests of a project in parallel worker processes.

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
- `tondo test --filter text`, `--exact id` (again for more), `--list`, `--jobs n`,
  `--timeout s`, `--retry n`, `--order random --seed hex`,
  `--report junit=out.xml`; `--json` gives the report as data, and
  `--test-format events` a line of JSON for each result as it comes.
- A test that calls `testing.bench` in its body is a benchmark (below).
- A test that ends with tasks still running fails, with where each waits and
  where it was spawned: wait for them, cancel them, or close what they wait on.
- `tondo test --schedules 50` runs each test with its tasks in 50 orders, to
  find one that depends on which task runs first; a failure prints the
  command that runs that order again, `--schedules 1 --seed hex`.

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
  optimized: each warms up, takes 50 samples in `--bench-time` seconds (3 by
  default) and counts what a run allocates, then prints the time a run
  takes, its spread, and its allocations and bytes:
  `parse 10  1.973 µs/op  ±0.5%  51 allocs/op  1685 B/op`.
  `--test-format json` gives every sample, and the machine.
- What the closure gives is kept, so the work that made it is not optimized
  away; `testing.keep(value)` keeps a value the closure does not give.
