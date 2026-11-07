# Runbook: testing a change, with every kind of test

Tondo has a tool for each question a test can answer. Most of them need no
setup, and each finds what the others miss, so a change is tested best by a
few of them together, cheapest first.

| The question | What answers it |
| --- | --- |
| Does it do what I meant, for these inputs? | a test with `assert`, beside the code |
| Do the errors say why? | the same test, comparing `err(...)` |
| Is there an input that breaks it? | `testing.fuzz`, and `test` with `fuzz` to search |
| Does a state machine keep its rules after any events, in any order? | `model.machine`, written by the tool `model` |
| Do tasks that share a value get it right in every order? | `test`, which tries them by itself; `--schedules all` for those that reach outside |
| Does a protocol or a design keep its properties? | `model.check`, `model.watch`, `model.follows` (`tondo guide models`) |
| Does an output keep its shape? | `testing.snapshot` |
| What did no test run? | `test` with `coverage` |
| How fast is it, and did a change slow it? | `testing.bench`, `test` with `bench`, `against` |
| Which tests does a change reach? | `test` with `affected`, or `--watch` |

## The story: an order, from its first test to every order of its tasks

An order is paid, shipped or cancelled, by events a client writes, and tasks
handle the events of one order at once.

```sh
tondo init
rm src/main_test.to
```

```tondo file=src/main.to
fn main() {}
```

```tondo file=src/order.to
import std.sync

pub enum Event {
    Pay
    Ship
    Cancel
}

pub type Order = {
    paid: Bool
    shipped: Bool
    cancelled: Bool
}

/// The order after an event, or why the event cannot happen to it.
pub fn apply(order: Order, event: Event): Order ! String {
    match event {
        Event.Pay if order.paid => fail "already paid"
        Event.Pay => order with { paid: true }
        Event.Ship if not order.paid => fail "not paid"
        Event.Ship => order with { shipped: true }
        Event.Cancel => order with { cancelled: true }
    }
}

/// An event as a client writes it: `pay`, `ship` or `cancel`.
pub fn parse(text: String): Event ! String {
    match text.trim().toLower() {
        "pay" => Event.Pay
        "ship" => Event.Ship
        "cancel" => Event.Cancel
        other => fail "no event is called `{other}`"
    }
}

/// An event that comes for an order that tasks share: whether it applied.
pub fn handle(order: sync.Mutex[Order], event: Event): Bool {
    match apply(order.get(), event) {
        ok(next) => {
            order.set(next)
            true
        }
        err(_) => false
    }
}
```

The tests go beside the code, in `src/order_test.to`, where they see what the
module keeps private. Each says one thing the code does, by its name; a test
fails with an error as with an `assert`, so `?` reads what it needs.

```file src/order_test.to
import std.sync
import std.testing

fn fresh(): Order {
    Order { paid: false, shipped: false, cancelled: false }
}

test shipsOnlyWhatIsPaid {
    assert(apply(fresh(), Event.Ship) == err("not paid"))
    let paid = apply(fresh(), Event.Pay)?
    assert(apply(paid, Event.Ship) == ok(paid with { shipped: true }))
}

test readsWhatClientsWrite {
    for (text, event) in [("pay", Event.Pay), (" Ship\n", Event.Ship), ("CANCEL", Event.Cancel)] {
        assert(parse(text) == ok(event), "{text}")
    }
    assert(parse("refund").isErr())
}

// Text comes from outside: no text may make it panic.
test anyTextIsAnEventOrSaysWhy {
    testing.fuzz("parse", ["pay", "", "Ship "], (text: String) {
        _ = parse(text)
    })?
}

test applying {
    testing.bench("apply", () { apply(fresh(), Event.Pay) })?
}

test payAndCancelAtOnce {
    let order = sync.Mutex(fresh())
    let paying = spawn handle(order, Event.Pay)
    let cancelling = spawn handle(order, Event.Cancel)
    assert(paying.wait() and cancelling.wait())
    assert(order.get() == Order { paid: true, shipped: false, cancelled: true })
}
```

`check` reads the tests too, once the code is right. Then `test` runs them all:
the fuzz test with its seeds and the benchmark once. A test that starts tasks
and stays in the program, as `payAndCancelAtOnce`, it also runs in every order
of its tasks that differs, a few hundred at most, since one run proves only the
order the machine gave: here one of them loses a write, in the order the
machine gave or in another, and the run names it either way.

```mcp
check {}
=> it compiles
test {}
=> shipsOnlyWhatIsPaid ... ok
=> payAndCancelAtOnce
=> that write is lost
```

`apply` gives the next state for an event: it is a state machine, and the
tool `model` writes the test that goes through every state it reaches with
every event, starting from a state built from its type. Nothing breaks yet,
because nothing says what must hold.

```mcp
model {"function": "apply"}
=> applyIsAStateMachine
```

The rule goes in `holds`: an order is never both shipped and cancelled.

```edit src/order_test.to
-    model.expect(model.machine(start, apply))
+    let rules = model.always((order: Order) { not (order.shipped and order.cancelled) }, name: "never shipped and cancelled")
+    model.expect(model.machine(start, apply, holds: [rules]))
```

```mcp
test {"filter": "applyIsAStateMachine"}
=> `never shipped and cancelled` does not hold
=> Cancel: Order { paid: true, shipped: true, cancelled: true }
```

It gives the shortest way there: paid, shipped, then cancelled. It tries the
events in every order, so the fix closes both ways in, a cancel after shipping
and a shipping after a cancel, or the next run gives the other.

```edit src/order.to
-        Event.Ship => order with { shipped: true }
-        Event.Cancel => order with { cancelled: true }
+        Event.Ship if order.cancelled => fail "cancelled"
+        Event.Ship => order with { shipped: true }
+        Event.Cancel if order.shipped => fail "already shipped"
+        Event.Cancel => order with { cancelled: true }
```

Back to `payAndCancelAtOnce`: `handle` reads the order and writes it in two
steps, and the other task's write falls between them. Read and write it in one
step:

```edit src/order.to
-    match apply(order.get(), event) {
-        ok(next) => {
-            order.set(next)
-            true
-        }
-        err(_) => false
-    }
+    order.update((now) {
+        match apply(now, event) {
+            ok(next) => {
+                now = next
+                true
+            }
+            err(_) => false
+        }
+    })
```

```mcp
test {}
=> 6 passed
=> passed in every order of them that differs
```

`coverage` says what no test ran: one line of `src/order.to` of 19, where
`handle` refuses an event. A test of it:

```mcp
test {"coverage": true}
=> lines 94.7% (18/19)
```

```edit src/order_test.to
-test applying {
+test refusesWhatCannotHappen {
+    let order = sync.Mutex(fresh())
+    assert(not handle(order, Event.Ship))
+}
+
+test applying {
```

```mcp
test {"coverage": true}
=> lines 100.0% (19/19)
```

Last, what is slower to run, and so run on purpose: `fuzz` searches for
seconds for text that breaks `parse`, and keeps in `fuzz/` what fails, which
every later `test` tries again; `bench` measures `apply`.

```mcp
test {"fuzz": 2}
=> anyTextIsAnEventOrSaysWhy ... ok
test {"bench": true, "bench_time": 1}
=> ns/op
```

## Each of them, and when

- **A test beside the code** is where everything starts: `name_test.to` next
  to `name.to`, a test for each thing the code does, named for it
  (`shipsOnlyWhatIsPaid`, not `test1`). Compare whole values, `==` on a
  record or on `ok(...)` and `err(...)`, rather than parts of them or text.
  A test in `tests/` sees only what is public, as a user of the package.
- **The errors** are part of what the code does: test that a bad input gives
  the error it should, not only that it fails. `recover { ... }` tests a panic.
- **Fuzzing**: any function that takes outside input, text, bytes, a request,
  gets a test with `testing.fuzz` and a few seeds. `test` runs the seeds and
  what failed before; `fuzz` with seconds searches, and keeps what it finds in
  `fuzz/`, to commit. Assert what must hold of every input, as a value that
  encodes and decodes back to itself. `tondo guide runbook-fuzz`.
- **A state machine**: logic that moves between states on events is best
  written as `fn apply(state: S, event: E): S ! Error`, which `model.machine`
  checks with no model to write, and `model` writes the test of. Then each
  rule is a property in `holds`. Prefer this to many tests of single events.
- **Every order**: a test where tasks share something, small, with two or
  three tasks. `test` runs it in every order of them that differs, up to a
  few hundred, when it stays in the program; one that reaches files, the
  network or a database runs once, as running it again would change them,
  and `--schedules all` tries its orders when it leaves them as it found
  them. A failure gives the order, to run again and to debug with
  `debug_start` and `schedule`. `tondo guide runbook-flaky`.
- **Models**: when the design itself is the risk, a protocol, a scheduler, a
  lock, before or beside the code. `model.watch` holds the real code to
  properties at each `model.mark`; `model.check` explores a model, and
  `model.follows` holds the code to it. `tondo guide models`.
- **Snapshots**: an output too big to write in a test, as a page or a
  document: `testing.snapshot(name, value)`, recorded with `update_snapshots`
  and then compared. Read what it records before you commit it.
- **Time**: code that sleeps or waits for a timeout is tested in
  `testing.withVirtualTime`, where time moves when the test says, not by
  sleeping. A test never sleeps to wait for a task: it waits for it.
- **Coverage** after the tests are written, to find what none runs: an
  error path, a branch. A line no test runs is a question, not a target:
  test it, or ask why it is there.
- **Benchmarks** beside the tests of the code they measure; `test` runs each
  once, so they cannot rot. `bench` with `save_baseline` keeps a measure, and
  `against` with `max_regression` fails a change that slows it.
  `tondo guide runbook-performance`.

## Together, in this order

1. `check` after each edit, which reads the tests too.
2. `test` after each change; with `affected` and a commit, only the tests that
   reach what changed, and with `--watch` again on each save.
3. The tests that need more, when the code they test changes: `model` for a
   state machine, `--schedules all` for tasks that reach outside the program,
   `fuzz` for a parser, `bench` for a hot path.
4. `coverage`, then the tests it shows are missing.
5. Before calling it done: `test` with no filter, all passing.

When one fails, the runbooks say what to do: `runbook-flaky` for a test that
fails sometimes, `runbook-regression` to keep a failure from coming back,
`runbook-debug` for a value that is wrong. In CI, run `tondo test`, then
`--schedules all` on the tests of tasks that reach outside the program and
leave it as they found it, `--models` for the models, `--bench
--against main --max-regression 5` for speed, and `--fuzz --fuzz-time 300`
each night.

## What not to do

- Do not run a test many times to show it passes: `test` tries the orders
  that matter, and says when it tried them all.
- Do not make a test pass by weakening it: a failing `assert` is the finding.
- Do not sleep in a test, nor mock the clock by hand: `withVirtualTime`.
- Do not write a model of code that is a state machine: `model.machine` checks
  the code itself, and cannot drift from it.
- Do not compare printed text when a value can be compared.
- Do not put the code a fuzz test tries in the test file: the search counts
  as new ground the code of the project, of its dependencies and of the
  standard library, not the tests'.
