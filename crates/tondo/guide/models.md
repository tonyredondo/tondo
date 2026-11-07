# Models

`std.model` goes through every state a design or a piece of code can reach,
in every order of its steps, and says what breaks what must hold, step by step,
with each state on the way. It runs in a test, as anything else does.

Try these in order: the first two need no model at all, so there is nothing to
keep in step with the code.

1. The code is a state machine: `model.machine` checks the production function.
2. The code runs tasks: `model.watch` checks properties of the real code.
3. Otherwise, write a model, and keep it honest with `model.mark` and
   `model.follows`.

## A state machine: no model to write

A function that gives the next state for an event is the model already.
`model.machine` tries every event in every state, every value of the event's
type there is, and stops at the first that breaks what `holds` says. An event
the function fails for is one that cannot happen there; one it panics for is a
bug, shown with the way to it. A function that never fails, `fn(S, E): S`,
passes too.

```tondo test
import std.model

enum Event {
    Insert
    Turn
    Push
}

type Gate = {
    paid: Bool
    open: Bool
}

fn apply(gate: Gate, event: Event): Gate ! String {
    match event {
        Event.Insert => gate with { paid: true }
        Event.Turn if gate.paid => gate with { open: true, paid: false }
        Event.Turn => fail "not paid"
        Event.Push if gate.open => gate with { open: false }
        Event.Push => fail "closed"
    }
}

test theGateOpensOnlyWhenPaid {
    let start = Gate { paid: false, open: false }
    let paid = model.always((gate: Gate) { not (gate.open and gate.paid) }, name: "never open and paid")
    model.expect(model.machine(start, apply, holds: [paid]))
}
```

`model.expect` gives what was checked, as `model.expect(...).states`, or fails
the test with what broke. `tondo test` also says which
events no state took, which is often a mistake. An event that holds a number or
a string has no end of values: then `events: (gate) { [...] }` says which can
happen in a state.

## What must hold

`holds` takes properties, by what they say:

- `model.always(p)`: every state reached is one `p` holds of.
- `model.eventually(p)`: every run gets to such a state.
- `model.leadsTo(p, q)`: whenever `p` holds, `q` does then or later.
- `model.infinitelyOften(p)`, `model.eventuallyAlways(p)`: again and again,
  and from some point on for ever.
- `model.reachable(p)`: from every state such a state can still be reached,
  which finds a wait nobody ends.
- `model.temporal(formula)`: anything else, as a formula of linear temporal
  logic built from `model.state(p)` with `next`, `always`, `eventually`,
  `until`, `implies`, `negated`, `model.allOf` and `model.anyOf`.

Each takes a `name`, which the failure says, and `broken.property` gives, to
test which broke. A property can hold with nothing to say, as a `leadsTo`
whose `when` held in no state: `tondo test` says so, `model: ... says
nothing`, and so of a model that never leaves its start, and
`checked.vacuous` names them. Read it as a model that does not reach what
you meant. A run takes a step whenever one is possible, and one with no
step left stays where it is. A loop that never gets where it must breaks
`eventually`, shown as `and then round for ever:`; a step that cannot be put
off for ever goes in `fair`: `model.weak("grant")` if it comes when it stays
possible, `model.strong("grant")` if it comes when it keeps becoming possible.
`ends: (state) { ... }` says where a run may stop: anywhere else, stopping is a
deadlock.

Fairness is of a step's name: `model.weak("resume")` holds when some task
resumes, even if it is always the same one. To say that each task resumes, put
the task in the name, `model.step("resume {t}", ...)`, and make each fair:
`[0, 1].map((t) { model.weak("resume {t}") })`.

## Concurrent code: properties of the real thing

`model.watch` runs the code and holds the properties to the state it reads at
the start, at each `model.mark`, and at the end. Under
`tondo test --schedules all` it does so in every order of the code's tasks.

```tondo test
import std.model
import std.sync

fn adds(count: sync.Atomic[Int]) {
    _ = count.add(1)
    model.mark("add")
}

test bothAdd {
    let count = sync.Atomic(0)
    let run = () {
        let a = spawn adds(count)
        let b = spawn adds(count)
        a.wait()
        b.wait()
    }
    let both = model.eventually((n: Int) { n == 2 }, name: "both add")
    model.expect(model.watch(() { count.load() }, [both], run))
}
```

`model.mark` does nothing outside the build of tests, and costs nothing there.

## A model, kept honest

A model of a design before its code, or of code too big to go through, is a
start state and the steps that can follow each one: `model.check`. The code
marks the steps it takes with `model.mark`, and `model.follows` holds what it
marked to the model, in every order under `--schedules all`. If one changes and
the other does not, the test fails at the step where they part, and
`tondo test` names the steps of the model the code never takes in any order.
Before anything runs, `check` says of a `model.mark("gave")`, or a
`model.weak("gave")`, that no `model.step` of the program is named so
(`W1020`), with the nearest.

```tondo test
import std.model
import std.sync

type Lock = {
    holder: Int?
}

fn steps(lock: Lock): Array[model.Step[Lock]] {
    match lock.holder {
        none => [model.step("take", Lock { holder: some(1) })]
        some(_) => [model.step("give", Lock { holder: none })]
    }
}

fn worker(lock: sync.Mutex[Int]) {
    lock.update((value) {
        model.mark("take")
        value += 1
        model.mark("give")
    })
}

test theLockFollowsItsModel {
    model.expect(model.check(Lock { holder: none }, steps, holds: [model.reachable((lock: Lock) { lock.holder.isNone() })]))
    let both = () {
        let lock = sync.Mutex(0)
        let a = spawn worker(lock)
        let b = spawn worker(lock)
        a.wait()
        b.wait()
    }
    model.expect(model.follows(Lock { holder: none }, steps, both))
}
```

A state is any value that encodes, arrays and maps included, and two states are
the same when they encode the same; a map is the same state whatever order its
keys came in. A list of tasks is an `Array`, and
`lock with { tasks: lock.tasks.replaced(t, task) }` is the state with one of
them changed. A
queue is a `model.Queue([1, 2])`, and a collection whose order says nothing a
`model.Bag(...)`. `symmetry: (state) { ... }` gives the one state that stands
for all those it equals, as the same with its tasks numbered otherwise, so that
only it is gone through. `model.every[T]()` gives every value of a type.

A big model goes faster with `workers: 8`, tasks that work out what follows
each state at once, with the same result as one; and one too big for memory
goes to `disk: some("target/model")`, where its states are written a level of
the search at a time, with only a fingerprint of each kept in memory, as TLC
does.

## Running them

- `tondo model apply` writes the test of the state machine `apply`, next to
  its file, with a start state built from its type, and runs it; again, it
  runs the test as the program left it. The editor offers it as a lens over
  each function that is one, *Check as a state machine*, and `tondo mcp` as
  the tool `model`.
- `tondo test --models` runs the tests that check a model; `--schedules all`
  goes through every order of their tasks.
- The report of `--test-format json` has each model in `models`: how many
  states and steps, the events or steps never taken, and what broke.
