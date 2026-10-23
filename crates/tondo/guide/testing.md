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
- `tondo test --filter text`, `--exact id`, `--list`, `--jobs n`,
  `--timeout s`, `--retry n`, `--order random --seed hex`,
  `--report junit=out.xml`.
- A test that ends with tasks still running fails, with where each waits and
  where it was spawned: wait for them, cancel them, or close what they wait on.
- `tondo test --schedules 50` runs each test with its tasks in 50 orders, to
  find one that depends on which task runs first; a failure prints the
  command that runs that order again, `--schedules 1 --seed hex`.
