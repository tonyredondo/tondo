# Testing

Tests are part of the language: `test` and `suite` are keywords, a test sits
beside the code it tests, and `tondo test` runs them. This chapter covers
writing tests of every kind. What the runner does with them, coverage,
reports and the rest, is in [the toolchain](../toolchain.md).

- [A test](#a-test)
- [Where tests live](#where-tests-live)
- [assert](#assert)
- [Tests that fail with an error](#tests-that-fail-with-an-error)
- [Testing that code panics](#testing-that-code-panics)
- [Suites](#suites)
- [What a test tells the runner](#what-a-test-tells-the-runner)
- [Snapshots](#snapshots)
- [Time without waiting](#time-without-waiting)
- [Tests of tasks](#tests-of-tasks)
- [Benchmarks](#benchmarks)
- [Fuzzing](#fuzzing)
- [Models](#models)
- [Testing a server](#testing-a-server)
- [Which test for which question](#which-test-for-which-question)

## A test

```tondo test
fn parse(text: String): Int ! String {
    Int.parse(text.trim()).mapErr((_) { "`{text}` is not a number" })
}

test parsesANumber {
    assert(parse(" 42 ") == ok(42))
}

test rejectsText {
    assert(parse("x") == err("`x` is not a number"))
}
```

A `test` has a name and a body. It passes when its body ends, and fails when
an `assert` does not hold, when it panics, or when it lets an error out.
`tondo test` runs every test of a project:

```text
test calc::unit::calc::parsesANumber ... ok
test calc::unit::calc::rejectsText ... ok
ok. 2 passed; 0 failed; 0 skipped; 0 blocked; 0.00s
```

Each test has an identifier: its package, whether it is a unit or an
integration test, its module, its suites, and its name. `tondo test --filter
parses` runs those whose identifier has the text, `--exact <id>` one, and
`--list` names them all.

Tests do not depend on each other. Each runs alone, in an order that is not
promised, in parallel worker processes, so a test that needs another to
have run first is a broken test.

## Where tests live

```text
shop/
  src/
    cart/
      cart.to              the code
      cart_test.to         unit tests of the module shop.cart
  tests/
    checkout.to            an integration test
```

- A file whose name ends in `_test.to` is part of the module it sits in. It
  sees everything the module has, private functions and fields included,
  with nothing to import. This is where most tests go.
- A file under `tests/` is a program of its own that imports the package as
  any user would, and sees only what is `pub`. It tests the package from
  outside, and so tests its API.

`test`, `suite` and `import std.testing` are allowed only in those files.
Test code is never part of what `tondo build` makes.

## assert

`assert(condition)` fails the test when the condition is false. For a
comparison it shows both sides, so the message rarely needs writing:

```text
--- calc::unit::calc::arithmetic::edges::wrong
panic[P0007]: assertion-failed: mean(values) == 5
  left:  4
  right: 5
 --> src/main_test.to:55:13
stack:
  test wrong  src/main_test.to:55:13
```

A second argument adds a message, for what the values do not say:
`assert(user.isActive(), "{user.name} should be active after the login")`.

There is one `assert`, and no family of `assertEquals`, `assertContains` and
the rest: any expression that gives a `Bool` is a check, and `==` works on
every value, records and collections included.

```tondo test
type User = { name: String, roles: Array[String] }

fn promote(user: User): User {
    user with { roles: user.roles.concat(["admin"]) }
}

test promotingAddsARole {
    let ada = User { name: "ada", roles: ["editor"] }
    let promoted = promote(ada)
    assert(promoted == User { name: "ada", roles: ["editor", "admin"] })
    assert("admin" in promoted.roles)
    assert(ada.roles == ["editor"], "the original was changed")
}
```

## Tests that fail with an error

A test may use `?`. An error that reaches the end of the test fails it, with
the error:

```tondo test
import std.json

type Settings = { name: String, port: Int = 8080 }

test readsTheDefaultPort {
    let settings = json.decode[Settings]("""
        {{ "name": "api" }}
        """)?
    assert(settings.port == 8080)
}
```

This keeps tests about what they check. A step that must work and is not
the point of the test is one line with a `?`, not a `match`.

## Testing that code panics

`recover { ... }` gives the panic as a value, so a test can check that a bug
is caught, and how:

```tondo test
import std.testing

fn mean(values: Array[Int]): Int {
    assert(not values.isEmpty(), "the mean of no values")
    values.sum() / values.length()
}

test theMeanOfNothingIsABug {
    match recover { mean([]) } {
        err(crash) => assert(crash.code == "P0007")
        ok(value) => testing.failNow("it gave {value}")
    }
}

test overflowIsCaught {
    assert(recover { Int.max + 1 }.isErr())
}
```

## Suites

A `suite` groups tests, and can prepare something for them. What its body
declares before its tests is their setup, and its `defer`s are their
teardown:

```tondo test
import std.testing

fn mean(values: Array[Int]): Int {
    values.sum() / values.length()
}

suite arithmetic {
    let values = [2, 4, 6]              // setup, which the tests below see
    defer testing.log("arithmetic is done")

    test mean {
        assert(mean(values) == 4)
    }

    suite edges {
        test one {
            assert(mean([5]) == 5)
        }
    }
}
```

- A suite's statements run once for the tests of that suite, before them,
  and its `defer`s after them. A test still starts from the same values
  whether it runs first or last.
- Suites nest, and a test's identifier has them in order:
  `calc::unit::calc::arithmetic::edges::one`.
- A suite is for tests that share a costly setup, as a database with its
  schema, or simply belong together. Tests that share nothing need none.

## What a test tells the runner

`std.testing` is how a test speaks to the runner:

| Call | Does |
| --- | --- |
| `testing.log(message)` | adds a line to the test's report, shown when it fails |
| `testing.skip(reason)` | ends the test as skipped, with the reason |
| `testing.failNow(message)` | fails the test at once |
| `testing.tags(["area": "billing"])` | labels the test in its report |
| `testing.attach(name, mediaType, bytes)` | adds a file to the report, as a screenshot or a response |
| `testing.snapshot(name, value)` | compares a value with a recorded one |
| `testing.withVirtualTime(...)` | runs code that sleeps, without waiting |
| `testing.bench(name, closure)` | makes the test a benchmark |
| `testing.fuzz(name, seeds, closure)` | tries the code with inputs |
| `testing.keep(value)` | in a benchmark, stops the optimizer from dropping work |

```tondo test
import std.env
import std.testing

test needsTheDatabase {
    if env.get("DATABASE_URL").isNone() {
        testing.skip("set DATABASE_URL to run it")
    }
    testing.tags(["area": "storage"])
    testing.log("connecting")
}
```

```text
test calc::unit::calc::needsTheDatabase ... skipped: set DATABASE_URL to run it
```

## Snapshots

A snapshot records a value in a file, and from then on the test fails when
the value changes. It suits output that is large and should stay as it is: a
report, a rendered page, the reply of an API.

```tondo test failing
import std.testing

type Report = { title: String, lines: Array[String], total: Int }

fn report(values: Array[Int]): Report {
    Report { title: "values", lines: values.map((value) { "{value}" }), total: values.sum() }
}

test reportsAsBefore {
    testing.snapshot("report", report([1, 2, 3]))
}
```

The first run has nothing to compare with and says so. `tondo test
--update-snapshots` records the value, as JSON, in
`snapshots/unit/<module>/<test>/report.snap`, which is committed with the
code. A later change shows as the lines that differ:

```text
panic[P2007]: test-snapshot-mismatch: the snapshot "report" differs; `tondo test --update-snapshots` records the new value
snapshot report: mismatched
  line 7, expected:   "title": "values",
  line 7, found:      "title": "numbers",
```

Parts that change in every run, as a time or an identifier, are left out,
matched against a pattern or replaced, with the `ignore`, `matches` and
`replace` arguments of `snapshot`. Read a snapshot's diff before updating
it: recording a wrong value makes the test agree with the bug.

## Time without waiting

Code that sleeps, retries or times out would make tests slow, and flaky on a
busy machine. `testing.withVirtualTime` gives the body a clock that moves
only when told:

```tondo test
import std.testing
import std.time

fn retry(attempts: Int): Int {
    var tries = 0
    for tries < attempts {
        time.sleep(time.seconds(30))        // half a minute between tries
        tries += 1
    }
    tries
}

test retriesThreeTimes {
    testing.withVirtualTime((clock) {
        let job = spawn retry(3)
        clock.advance(time.seconds(90))     // a minute and a half, at once
        assert(job.wait() == 3)
    })
}
```

The test takes no time. `clock.advance(d)` moves the clock and runs every
task whose time has come on the way, and `clock.settle()` lets the tasks
that are ready run until all of them wait. A test never waits for the real
clock, and never asserts that something took less than some real time.

## Tests of tasks

A test that starts tasks passes or fails depending on the order they run
in, and a single run shows one order. `tondo test` runs such a test again in
every order of its tasks that differs, and reports the one that fails with
the command that repeats it:

```tondo test
import std.sync

fn deposit(balance: sync.Mutex[Int], amount: Int) {
    balance.update((value) { value += amount })
}

test depositsAddUp {
    let balance = sync.Mutex(0)
    let first = spawn deposit(balance, 10)
    let second = spawn deposit(balance, 5)
    first.wait()
    second.wait()
    assert(balance.get() == 15)
}
```

```text
note: 1 test that starts tasks passed in every order of them that differs
```

There is nothing to write for this. A test of concurrent code should be
small, start the tasks that share something, and assert what must hold at
the end; the runner supplies the orders. A test also fails if it ends with
tasks still running, and says where each one waits.

## Benchmarks

A test that calls `testing.bench` in its own body is a benchmark:

```tondo test
import std.json
import std.testing

type Point = { x: Int, y: Int }

test encoding {
    for size in [10, 1_000] {
        let points = (0..size).map((n) { Point { x: n, y: n * 2 } }).collect()
        testing.bench("encode {size} points", () { json.encode(points) })?
    }
}
```

A plain `tondo test` runs each closure once, so a benchmark that breaks
fails like any test. `tondo test --bench` measures them, optimized: the
time of a run with its spread, what it allocates, and on Linux the
instructions it executes:

```text
  count 600 words     38.56 µs/op  ±6.7%  8 allocs/op  25712 B/op  445.5k ins/op
```

What the closure returns is kept, so the work that made it is not optimized
away. Set up the input outside the closure, so that only what is being
measured is inside it.

## Fuzzing

A test that calls `testing.fuzz` checks a property for inputs it did not
choose. The input may be any type the program can encode: a string, bytes,
or a record of its own.

```tondo test
import std.json
import std.testing

type Order = { id: Int, items: Array[String], note: String? }

test ordersSurviveARoundTrip {
    let seeds = [Order { id: 1, items: ["tea"], note: none }]
    testing.fuzz("round trip", seeds, (order: Order) {
        let text = json.encode(order)
        assert(json.decode[Order](text) == ok(order))
    })
}
```

A plain `tondo test` runs the closure with each seed and with every input
that failed before. `tondo test --fuzz` searches for new ones, changing
inputs knowing their type and keeping those that reach new code; an input
that fails is reduced and saved under `fuzz/`, to commit, and is a test from
then on.

Write the closure as a property: something that holds for every input, as
"decoding what was encoded gives it back", "sorting twice is sorting once",
or "this never panics". Input the code is right to reject is handled inside
the closure.

## Models

`std.model` checks more than examples: every state a piece of code can
reach, in every order of its steps. A function that gives the next state for
an event needs nothing else:

```tondo test
import std.model

enum Event {
    Insert
    Turn
    Push
}

type Gate = { paid: Bool, open: Bool }

fn apply(gate: Gate, event: Event): Gate ! String {
    match event {
        Insert if gate.open => fail "already open"
        Insert => gate with { paid: true }
        Turn if gate.paid => gate with { open: true, paid: false }
        Turn => fail "not paid"
        Push if gate.open => gate with { open: false }
        Push => fail "closed"
    }
}

test theGateOpensOnlyWhenPaid {
    let start = Gate { paid: false, open: false }
    let rule = model.always((gate: Gate) { not (gate.open and gate.paid) }, name: "never open and paid")
    model.expect(model.machine(start, apply, holds: [rule]))
}
```

`model.machine` tries every event in every state the gate can be in, and
when a rule breaks it gives the events that lead there. `tondo model apply`
writes this test for a function. `model.watch` holds rules to concurrent
code in every order of its tasks, and `model.check` checks a design that has
no code yet. `tondo guide models` goes through them, and
[`std.model`](../std/model.md) lists every function.

## Testing a server

A server is tested by starting it. `app.start()` serves in a task of its
own, on a free port, and gives a `Server` with its `url()`:

```tondo test
import std.http

type User = { id: Int, name: String }

fn find(id: Int): User? {
    if id == 1 { User { id, name: "Ada" } } else { none }
}

fn routes(): http.App {
    var app = http.App()
    app.get("/users/:id", (id: Int) { find(id) })
    app
}

test findsAUser {
    let server = routes().start()?
    defer server.stop()
    let user: User = http.get("{server.url()}/users/1")?.json()?
    assert(user == User { id: 1, name: "Ada" })
}

test aMissingUserIsA404 {
    let server = routes().start()?
    defer server.stop()
    match http.get("{server.url()}/users/2") {
        err(Status(response)) => assert(response.status() == 404)
        other => assert(other.isErr(), "it answered")
    }
}
```

`defer server.stop()` ends the server with the test, which would otherwise
fail for leaving a task running. The rest of the library is as easy to put
in a test: `sqlite::memory:` is a database that lives in it, `fs.memory()` a
tree of files that never touches the disk, and
`fs.createTemporaryDirectory()` a directory for the tests that need a real
one.

## Which test for which question

| The question | What answers it |
| --- | --- |
| Does it do what I meant, for these inputs? | a `test` with `assert`, beside the code |
| Do its errors say why? | the same test, comparing `err(...)` |
| Is a bug caught as a bug? | `recover` in a test |
| Is there an input that breaks it? | `testing.fuzz` |
| Does a state machine keep its rules after any events? | `model.machine` |
| Do tasks that share a value get it right in every order? | a test that starts them; the runner tries the orders |
| Does a protocol keep its properties? | `model.check`, `model.watch` |
| Does an output keep its shape? | `testing.snapshot` |
| Does it behave over time? | `testing.withVirtualTime` |
| How fast is it, and did a change slow it? | `testing.bench` |
| Does the API work for someone outside? | a file under `tests/` |

Next: [Metaprogramming](17-meta.md).
