# std.model

Checks a design, or the code itself, in every state it can reach and in
every order of its steps, and gives the steps to the state that breaks a
rule. It runs in a test, like any other check.

There are three ways in, from the least work to the most:

| Function | Checks | Needs |
| --- | --- | --- |
| `model.machine` | a function that gives the next state for an event | nothing but the function |
| `model.watch` | properties of concurrent code, in every order of its tasks | the code, and marks where its state is read |
| `model.check` | a model written by hand, of a design that may have no code yet | a start state and the steps that follow each state |

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
        Event.Insert if gate.open => fail "already open"
        Event.Insert => gate with { paid: true }
        Event.Turn if gate.paid => gate with { open: true, paid: false }
        Event.Turn => fail "not paid"
        Event.Push if gate.open => gate with { open: false }
        Event.Push => fail "closed"
    }
}

test theGateOpensOnlyWhenPaid {
    let start = Gate { paid: false, open: false }
    let rule = model.always((gate: Gate) { not (gate.open and gate.paid) }, name: "never open and paid")
    let checked = model.expect(model.machine(start, apply, holds: [rule]))
    assert(checked.states == 3)
}
```

`model.machine` tries every event in every state. An event the function
fails for cannot happen there; one it panics for is a bug. When a rule
breaks, the test fails with the events that lead to the state:

```text
panic[P0008]: explicit-panic: `never open and paid` does not hold
  start: Gate { paid: false, open: false }
  Insert: Gate { paid: true, open: false }
  Turn: Gate { paid: false, open: true }
  Insert: Gate { paid: true, open: true }
```

The properties say what must hold: `always`, `eventually`, `leadsTo`,
`reachable`, `infinitelyOften`, `eventuallyAlways`, and `temporal` for any
formula of linear temporal logic. `tondo model <function>` writes the test
of a state machine and runs it, `tondo test --models` runs the tests that
check a model, and `tondo guide models` goes through all of it.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.model` shows the same in a terminal.

- [Functions](#functions): [`step`](#step), [`state`](#state), [`allOf`](#allof), [`anyOf`](#anyof), [`always`](#always), [`eventually`](#eventually), [`leadsTo`](#leadsto), [`infinitelyOften`](#infinitelyoften), [`eventuallyAlways`](#eventuallyalways), [`reachable`](#reachable), [`temporal`](#temporal), [`weak`](#weak), [`strong`](#strong), [`check`](#check), [`machine`](#machine), [`every`](#every), [`expect`](#expect), [`mark`](#mark), [`follows`](#follows), [`watch`](#watch)
- [Step](#step-1)
- [Checked](#checked)
- [Visit](#visit)
- [Violation](#violation)
- [Queue](#queue): [`Queue`](#queue-1), [`Queue.push`](#queuepush), [`Queue.first`](#queuefirst), [`Queue.rest`](#queuerest), [`Queue.items`](#queueitems), [`Queue.length`](#queuelength), [`Queue.isEmpty`](#queueisempty), [`Queue.contains`](#queuecontains), [`Queue.without`](#queuewithout)
- [Bag](#bag): [`Bag`](#bag-1), [`Bag.add`](#bagadd), [`Bag.remove`](#bagremove), [`Bag.items`](#bagitems), [`Bag.count`](#bagcount), [`Bag.contains`](#bagcontains), [`Bag.length`](#baglength), [`Bag.isEmpty`](#bagisempty)
- [Formula](#formula): [`Formula.negated`](#formulanegated), [`Formula.implies`](#formulaimplies), [`Formula.next`](#formulanext), [`Formula.always`](#formulaalways), [`Formula.eventually`](#formulaeventually), [`Formula.until`](#formulauntil)
- [Property](#property)
- [Fair](#fair)

### Functions

#### step

```tondo
pub fn step[S](name: String, next: S): Step[S]
```

The step `name`, which leads to `state`.

```tondo
import std.model

fn steps(count: Int): Array[model.Step[Int]] {
    if count < 2 { [model.step("add", count + 1)] } else { [] }
}

_ = steps(0)
```

#### state

```tondo
pub fn state[S](predicate: fn(S): Bool): Formula[S]
```

The formula that holds of a run whose first state `predicate` holds of.

#### allOf

```tondo
pub fn allOf[S](first: Formula[S], rest: ...Formula[S]): Formula[S]
```

Each of them holds.

#### anyOf

```tondo
pub fn anyOf[S](first: Formula[S], rest: ...Formula[S]): Formula[S]
```

One of them holds, at least.

#### always

```tondo
pub fn always[S](predicate: fn(S): Bool, name: String = "what must always hold"): Property[S]
```

Every state reached is one `predicate` holds of.

#### eventually

```tondo
pub fn eventually[S](
    predicate: fn(S): Bool,
    name: String = "what must come in the end",
): Property[S]
```

Every run gets to a state `predicate` holds of.

#### leadsTo

```tondo
pub fn leadsTo[S](
    when: fn(S): Bool,
    then: fn(S): Bool,
    name: String = "what must follow",
): Property[S]
```

Whenever `when` holds, `then` does later, or at once.

#### infinitelyOften

```tondo
pub fn infinitelyOften[S](
    predicate: fn(S): Bool,
    name: String = "what must keep coming",
): Property[S]
```

`predicate` holds again and again, for ever.

#### eventuallyAlways

```tondo
pub fn eventuallyAlways[S](predicate: fn(S): Bool, name: String = "what must settle"): Property[S]
```

From some state on, `predicate` always holds.

#### reachable

```tondo
pub fn reachable[S](
    predicate: fn(S): Bool,
    name: String = "what must stay within reach",
): Property[S]
```

From every state, one `predicate` holds of can still be reached, whatever
came before.

#### temporal

```tondo
pub fn temporal[S](formula: Formula[S], name: String = "the formula"): Property[S]
```

Every run is one `formula` holds of.

#### weak

```tondo
pub fn weak(name: String, each: Bool = false): Fair
```

A step that comes if it stays possible. With `each`, each step named
`name` and then a space and more, as each task's `"resume {t}"`: each one
comes, rather than one of them.

#### strong

```tondo
pub fn strong(name: String, each: Bool = false): Fair
```

A step that comes if it keeps becoming possible, even if not all the time;
with `each`, each step of that name, as `weak` says.

#### check

```tondo
pub fn check[S: reflect.Reflect + serialization.Encode + serialization.Decode](
    start: S,
    steps: fn(S): Array[Step[S]],
    holds: Array[Property[S]] = [],
    fair: Array[Fair] = [],
    ends: (fn(S): Bool)? = none,
    symmetry: (fn(S): S)? = none,
    workers: Int = 1,
    disk: String? = none,
): Checked ! Violation
```

Goes through every state `steps` reaches from `start`, nearest first, and
fails at the first one that breaks what `holds` says, or that `ends` says
should not end with no step left, with the shortest way there and each
state on it; then at the first property of what comes later that a run
breaks, with the loop that breaks it. A step in `fair` comes as it says
rather than be put off for ever. `symmetry` gives the one state that
stands for all those it equals, as the same with the tasks numbered
otherwise, so that only it is gone through. `workers` tasks work out what
follows each state at once, with the same result as one. With `disk`, a
directory, the states go there, a level of the search at a time, and in
memory stay only a fingerprint of each, as TLC keeps them, and the steps.

A run takes a step whenever one is possible; one that ends stays at its
last state.

```tondo
import std.console
import std.model

fn steps(count: Int): Array[model.Step[Int]] {
    if count < 3 { [model.step("add", count + 1)] } else { [] }
}

let holds = [model.always((count: Int) { count <= 3 }), model.eventually((count: Int) { count == 3 })]
match model.check(0, steps, holds: holds) {
    ok(checked) => console.println("{checked.states} states")
    err(broken) => console.println("{broken}")
}
```

#### machine

```tondo
pub fn machine[
    S: reflect.Reflect + serialization.Encode + serialization.Decode,
    E: reflect.Reflect + serialization.Decode,
    X,
](
    start: S,
    apply: fn(S, E): S ! X,
    events: (fn(S): Array[E])? = none,
    holds: Array[Property[S]] = [],
    fair: Array[Fair] = [],
    ends: (fn(S): Bool)? = none,
    symmetry: (fn(S): S)? = none,
    workers: Int = 1,
    disk: String? = none,
): Checked ! Violation
```

Goes through every state a state machine of the program reaches, as
`check` does, with the function that gives the next state for an event,
the same one the program calls: there is no model to keep in step with
the code. An event it fails for is one that cannot happen there; one it
panics for breaks the machine, and says how it got there.

Each event is tried in each state, every value of `E` that there is, as
`model.every` gives them, unless `events` says which can happen in a
state. `Checked.never` names those no state took.

```tondo
import std.console
import std.model

enum Event {
    Open
    Close
}

fn apply(open: Bool, event: Event): Bool ! String {
    match event {
        Event.Open if open => fail "already open"
        Event.Open => true
        Event.Close => false
    }
}

match model.machine(false, apply, holds: [model.infinitelyOften((open: Bool) { not open })]) {
    ok(checked) => console.println("{checked.states} states")
    err(broken) => console.println("{broken}")
}
```

#### every

```tondo
pub fn every[T: reflect.Reflect + serialization.Decode](): Array[T] ! String
```

Every value of `T` there is: each variant of an enum with each value of
what it holds, of a record each value of each field, of an option `none`
and each value, and of a `Bool` both. A number or a string has no end, and
a type that holds one fails with a message that says where.

```tondo
import std.console
import std.model

enum Light {
    Off
    On(Bool)
}

match model.every[Light]() {
    ok(all) => console.println("{all.length()} lights")
    err(why) => console.println(why)
}
```

#### expect

```tondo
pub fn expect(checked: Checked ! Violation): Checked
```

What was checked, or a failure of the test that calls it with what
broke, step by step.

```tondo
import std.model

fn steps(count: Int): Array[model.Step[Int]] {
    if count < 2 { [model.step("add", count + 1)] } else { [] }
}

let checked = model.expect(model.check(0, steps, holds: [model.always((count: Int) { count <= 2 })]))
assert(checked.states == 3)
```

#### mark

```tondo
pub fn mark(name: String)
```

Says that the code takes `step` of its model here, for `follows` and
`watch` to see in a test. Outside the build of tests it does nothing and
costs nothing.

```tondo
import std.model

fn grant(holder: Int): Int {
    model.mark("grant")
    holder
}

_ = grant(1)
```

#### follows

```tondo
pub fn follows[S: reflect.Reflect + serialization.Encode + serialization.Decode](
    start: S,
    steps: fn(S): Array[Step[S]],
    code: fn(),
): Checked ! Violation
```

Runs `code` and holds the steps it marks with `model.mark`, in the order
they came, to the model: each must be a step the model can take from where
the ones before led it, starting at `start`. A test under
`tondo test --schedules all` does so in every order of the code's tasks,
so that the code cannot do what the model says it cannot, and a change to
one that the other does not follow fails. `Checked.never` names the steps
of the model this run did not take.

```tondo
import std.console
import std.model

fn steps(open: Bool): Array[model.Step[Bool]] {
    if open { [model.step("close", false)] } else { [model.step("open", true)] }
}

fn door() {
    model.mark("open")
    model.mark("close")
}

match model.follows(false, steps, door) {
    ok(checked) => console.println("followed in {checked.steps} steps")
    err(broken) => console.println("{broken}")
}
```

#### watch

```tondo
pub fn watch[S: reflect.Reflect](
    observe: fn(): S,
    holds: Array[Property[S]],
    code: fn(),
): Checked ! Violation
```

Runs `code` and holds what `holds` says to the state `observe` gives at the
start, at each step the code marks with `model.mark`, and at the end, as a
run of those states: the properties of a model, on the code itself. A test
under `tondo test --schedules all` does so in every order of the code's
tasks. What comes later is of this run, which ends: `eventually` must come
before its end.

```tondo
import std.console
import std.model
import std.sync

let count = sync.Atomic(0)
let add = () {
    _ = count.add(1)
    model.mark("add")
}
let holds = [model.always((n: Int) { n <= 1 }), model.eventually((n: Int) { n == 1 })]
match model.watch(() { count.load() }, holds, add) {
    ok(checked) => console.println("{checked.states} states")
    err(broken) => console.println("{broken}")
}
```

### Step

```tondo
pub type Step[S] = {
    name: String
    state: S
}
```

What can happen from a state: its name, for the path to a fault and for
fairness, and the state it leads to.

```tondo
import std.console
import std.model

let step = model.step("add", 2)
console.println("{step.name} leads to {step.state}")
```

### Checked

```tondo
pub type Checked = {
    states: Int
    steps: Int
    depth: Int
    never: Array[String]
    /// The properties that held because what they are about never came, as
    /// a `leadsTo` whose `when` held in no state: they said nothing.
    vacuous: Array[String] = []
}
```

How much of a model was checked: the states it reaches, the steps between
them, the most steps from the start to any of them, and, of a `machine`,
the events no state takes, which are often a mistake.

```tondo
import std.console
import std.model

let checked = model.Checked { states: 3, steps: 2, depth: 2, never: ["Cancel"] }
console.println("{checked.states} states, {checked.depth} steps deep, never {checked.never}")
```

### Visit

```tondo
pub type Visit = {
    step: String
    state: String
}
```

A state on the way to what broke: the step that led there, empty for the
start, and the state as written.

```tondo
import std.console
import std.model

let visit = model.Visit { step: "open", state: "Door {{ open: true }}" }
console.println("{visit.step}: {visit.state}")
```

### Violation

```tondo
pub type Violation = {
    what: String
    path: Array[Visit]
    loop: Array[Visit]
    /// The name of the property of `holds` that broke; none for a state
    /// nothing can follow, a step that failed or code that strayed.
    property: String? = none
}
```

What broke, and how: the shortest way there, each step with the state it
led to, and, when it is a loop that never gets where it must, the loop.

```tondo
import std.console
import std.model

let start = model.Visit { step: "", state: "0" }
let broken = model.Violation { what: "it broke", path: [start], loop: [] }
console.println("{broken}")
```

Implements `Display`.

### Queue

```tondo
pub enum Queue[T] {
    Empty
    Item(T, Queue[T])
}
```

A queue a state can hold: what came first goes first, as a model says
of a queue of the code it checks.

```tondo
import std.console
import std.model

let waiting = model.Queue([1, 2]).push(3)
console.println("{waiting.first()} then {waiting.rest().length()} more")
```

Implements `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

#### Queue

```tondo
pub fn Queue[T](items: Array[T]): Queue[T]
```

A queue of the items, the first first.

#### Queue.push

```tondo
pub fn Queue[T].push(self, item: T): Queue[T]
```

The queue with `item` last.

#### Queue.first

```tondo
pub fn Queue[T].first(self): T?
```

What comes first, if anything does.

#### Queue.rest

```tondo
pub fn Queue[T].rest(self): Queue[T]
```

The queue without its first item.

#### Queue.items

```tondo
pub fn Queue[T].items(self): Array[T]
```

Its items, the first first.

#### Queue.length

```tondo
pub fn Queue[T].length(self): Int
```

How many items it holds.

#### Queue.isEmpty

```tondo
pub fn Queue[T].isEmpty(self): Bool
```

Whether it holds nothing.

#### Queue.contains

```tondo
pub fn Queue[T: Equatable].contains(self, item: T): Bool
```

Whether `item` is in it.

#### Queue.without

```tondo
pub fn Queue[T: Equatable].without(self, item: T): Queue[T]
```

The queue without the first `item` in it.

### Bag

```tondo
pub type Bag[T] = {
    // private fields
}
```

A collection whose order says nothing, which a state can hold: the same
items make the same bag however they came.

```tondo
import std.console
import std.model

let held = model.Bag([2, 1]).add(2)
console.println("{held.count(2)} of 2, {held == model.Bag([1, 2, 2])}")
```

Implements `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

#### Bag

```tondo
pub fn Bag[T: reflect.Reflect](items: Array[T]): Bag[T]
```

A bag of the items.

#### Bag.add

```tondo
pub fn Bag[T: reflect.Reflect].add(self, item: T): Bag[T]
```

The bag with `item` once more.

#### Bag.remove

```tondo
pub fn Bag[T: Equatable].remove(self, item: T): Bag[T]
```

The bag with one `item` fewer, if it held one.

#### Bag.items

```tondo
pub fn Bag[T].items(self): Array[T]
```

Its items, in an order of their own: the same for the same items.

#### Bag.count

```tondo
pub fn Bag[T: Equatable].count(self, item: T): Int
```

How many times `item` is in it.

#### Bag.contains

```tondo
pub fn Bag[T: Equatable].contains(self, item: T): Bool
```

Whether `item` is in it.

#### Bag.length

```tondo
pub fn Bag[T].length(self): Int
```

How many items it holds, each as many times as it is in it.

#### Bag.isEmpty

```tondo
pub fn Bag[T].isEmpty(self): Bool
```

Whether it holds nothing.

### Formula

```tondo
pub type Formula[S] = {
    // private fields
}
```

A formula of linear temporal logic over the states of a run, for what a
predicate of one state cannot say: `model.state(p).until(q)`, with `next`,
`always`, `eventually`, `implies`, `negated`, `model.allOf` and
`model.anyOf` nested as deep as it takes. `model.temporal` makes it a
property.

```tondo
import std.model

let positive = model.state((n: Int) { n > 0 })
let grows = positive.until(model.state((n: Int) { n > 9 })).always()
_ = model.temporal(grows, name: "it keeps positive until it passes 9")
```

#### Formula.negated

```tondo
pub fn Formula[S].negated(self): Formula[S]
```

It does not hold.

#### Formula.implies

```tondo
pub fn Formula[S].implies(self, other: Formula[S]): Formula[S]
```

Where this holds, `other` does too.

#### Formula.next

```tondo
pub fn Formula[S].next(self): Formula[S]
```

It holds from the next state on.

#### Formula.always

```tondo
pub fn Formula[S].always(self): Formula[S]
```

It holds of every state from here on.

#### Formula.eventually

```tondo
pub fn Formula[S].eventually(self): Formula[S]
```

It holds of some state from here on.

#### Formula.until

```tondo
pub fn Formula[S].until(self, other: Formula[S]): Formula[S]
```

It holds until `other` does, which comes.

### Property

```tondo
pub type Property[S] = {
    // private fields
}
```

What must hold of a model, which `check` takes in `holds`.

```tondo
import std.model

let positive = model.always((n: Int) { n >= 0 }, name: "never negative")
let ends = model.eventually((n: Int) { n == 3 })
_ = [positive, ends]
```

### Fair

```tondo
pub type Fair = {
    // private fields
}
```

A step that cannot be put off for ever, by its name: `weak` if it stays
possible, `strong` if it is possible again and again.

```tondo
import std.model

_ = [model.weak("tick"), model.strong("grant")]
```
