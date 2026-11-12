# Ownership: values, loans and identity

Tondo's memory has no rules to learn before writing a program: values are
copied, memory is managed, and there is no `null` or dangling reference to
meet. This chapter is about what is underneath that, for when it matters:
what a copy costs, how a function is let into a caller's value, the few
values that cannot be copied, and how to give something an identity.

- [Everything is a value](#everything-is-a-value)
- [What a copy costs](#what-a-copy-costs)
- [Lending: ref, mut and var](#lending-ref-mut-and-var)
- [Lending part of an array](#lending-part-of-an-array)
- [The rules of a loan](#the-rules-of-a-loan)
- [Values that cannot be copied](#values-that-cannot-be-copied)
- [Identity: Ref](#identity-ref)
- [WeakRef](#weakref)
- [How memory is managed](#how-memory-is-managed)
- [Resources outside the program](#resources-outside-the-program)
- [Sharing between tasks](#sharing-between-tasks)
- [Which to use](#which-to-use)

## Everything is a value

A number, a string, a record, an array, a map: each is a value. Assigning
one, passing it to a function, returning it and capturing it in a closure
all give the receiver its own, and the original stays as it was.

```tondo
import std.console

type Cart = { owner: String, items: Array[String] }

fn withGift(cart: Cart): Cart {
    var changed = cart
    changed.items.push("gift")
    changed
}

fn main() {
    let cart = Cart { owner: "ada", items: ["tea"] }
    var copy = cart
    copy.items.push("cup")
    let gifted = withGift(cart)
    console.println("{cart.items} {copy.items} {gifted.items}")
}
```

```text output
[tea] [tea, cup] [tea, gift]
```

There is no way for `withGift` to change the `cart` it was given, and none
for two variables to be secretly the same thing. Whoever holds a value is
the only one who can change it, which is why a value can be handed to
another function, another module or another task without thinking about
what they will do to it.

## What a copy costs

Very little. A string or a collection is copied by sharing its storage and
counting one more user. The storage is duplicated only if one of the users
changes it while another still holds it, and then once:

```tondo
import std.console

fn main() {
    let big = (1..=1_000_000).collect()
    let shared = big                // no elements copied
    let slice = big[1000:2000]      // nor here: a slice shares too
    var edited = big                // nor here
    edited[0] = -1                  // here, once: `edited` takes its own storage
    edited[1] = -2                  // and not again
    console.println("{big[0]} {shared[0]} {slice[0]} {edited[0]} {edited[1]}")
}
```

```text output
1 1 1001 -1 -2
```

A value held by one variable alone is changed in place, with no copy at
all, so a loop that pushes to an array it owns is as cheap as it looks. The
compiler also skips the count where it can prove nothing else sees the
value, and `tondo check --performance` warns of a loop that copies on every
turn.

Records and enums are copied by their fields. A small record is a few
machine words; a large one holds its strings and collections by sharing
them.

## Lending: ref, mut and var

Sometimes a function should work on the caller's value itself: to change it
in place, or to read a large one without even the count. The call then lends
the value, and both sides say so.

```tondo
import std.console

/// Reads without copying.
fn total(values: ref Array[Float]): Float {
    values.sum()
}

/// Changes the elements; the array keeps its length.
fn scale(values: mut Array[Float], factor: Float) {
    values *= factor
}

/// May also grow, shrink or replace it.
fn appendTotal(values: var Array[Float]) {
    values.push(values.sum())
}

fn main() {
    var numbers = [1.0, 2.0, 3.0]
    scale(mut numbers, 10.0)
    appendTotal(var numbers)
    console.println("{numbers} {total(ref numbers)}")
}
```

```text output
[10.0, 20.0, 30.0, 60.0] 120.0
```

| Mode | The function may | The caller writes | And needs |
| --- | --- | --- | --- |
| none | do anything to its own copy | `f(value)` | anything |
| `ref` | read | `f(ref value)` | anything |
| `mut` | change what is there | `f(mut value)` | a `var`, or a part of one |
| `var` | change, resize or replace it | `f(var value)` | a whole `var` |

The mode is in the signature and at the call, so reading either one says
what can happen. Nothing is lent by accident: a call without `mut` or `var`
cannot change its argument.

`mut` promises that the value keeps its shape, an array its length and a map
its keys, so it can be given a slice. `var` promises nothing, and takes only
a whole value.

Methods lend their receiver the same way: `self` is a `ref`, `mut self` a
`mut`, and `var self` a `var`. There the dot does the lending, which is why
`numbers.push(1.0)` needs no keyword at the call and does need `numbers` to
be a `var`.

A function that swaps a value for another can be written once, for any type:

```tondo
import std.console

fn replace[T](target: var T, replacement: T): T {
    var previous = replacement
    (previous, target) = (target, previous)
    previous
}

fn main() {
    var name = "old"
    let before = replace(var name, "new")
    console.println("{before} {name}")
}
```

```text output
old new
```

## Lending part of an array

A slice can be lent, and the function then works on that part of the
caller's array:

```tondo
import std.console

fn scale(values: mut Array[Float], factor: Float) {
    values *= factor
}

fn swapHalves(left: mut Array[Int], right: mut Array[Int]) {
    for index in 0..left.length() {
        (left[index], right[index]) = (right[index], left[index])
    }
}

fn main() {
    var numbers = [1.0, 2.0, 3.0, 4.0]
    let before = numbers                    // a copy, which stays as it was
    scale(mut numbers[1:3], 10.0)
    console.println("{numbers} {before}")

    var deck = [1, 2, 3, 4, 5, 6]
    swapHalves(mut deck[:3], mut deck[3:])  // two parts at once: they do not overlap
    console.println("{deck}")
}
```

```text output
[1.0, 20.0, 30.0, 4.0] [1.0, 2.0, 3.0, 4.0]
[4, 5, 6, 1, 2, 3]
```

Two parts lent with `mut` in one call must not share an element. When they
do, the call panics before the function starts (`P0004`):

```text
panic[P0004]: overlapping-borrow: one call lends two slices of an array that share the element at index 2
```

## The rules of a loan

- **A loan lasts for the call.** It cannot be stored in a field, returned,
  captured by a closure or sent to a task, so there is never a loan whose
  lifetime someone has to work out, and nothing to annotate.
- **While a value is lent with `mut` or `var`, nothing else touches it.**
  The caller's variable cannot be read or changed by another path until the
  call returns.
- **Several `ref` loans may overlap.** While any lasts, the value is not
  changed or moved.
- **A loan never crosses to another task.** `spawn` takes its arguments by
  value.

`for` lends the same way: `for mut item in items` lends each element for one
turn, and the collection for the whole loop
([Control flow](04-control-flow.md)). So does a pattern: `ref name` in a
`match` reads a part without copying it ([Enums and match](06-enums.md)).

Because a loan cannot be kept, the usual way to keep something is the
simple one: keep the value. It is yours, and copying it was cheap.

## Values that cannot be copied

A few values stand for exactly one thing outside themselves, and two of them
would make no sense: a task that is running, the sending end of a channel, a
group of tasks. These are not `Copy`. Assigning or passing one moves it, and
the old name can no longer be used:

```tondo error E1401
import std.console
import std.time

fn slow(): Int {
    time.sleep(time.milliseconds(1))
    7
}

fn main() {
    let job = spawn slow()
    let other = job
    console.println("{job.wait()} {other.wait()}")
}
```

```text
error[E1401]: `job` was moved before this use, since its type `Join[Int]` is not `Copy`; use it before passing it on
```

A record or a collection that holds such a value cannot be copied either,
and moves as a whole. Everything else in this chapter still applies: it can
be lent with `ref`, `mut` and `var`, and looked at with a `ref` pattern.

Generic code accepts only types that can be copied, unless it says
otherwise: a type parameter is `Copy` by default, since the usual generic
function copies its values freely. `T: ~Copy` lifts that, for a function
written to work on these values too:

```tondo
fn keep[T: ~Copy](value: T): T {
    value
}

fn main() {
    let job = spawn keep(1)
    let same = keep(job)
    _ = same.wait()
}
```

In everyday code this hardly comes up. The types that move are few, and
their own methods say what to do with them: `job.wait()`, `sender.copy()`.

## Identity: Ref

Two equal values are the same value. That is what is wanted nearly always,
and not when a program models things that are distinct though equal, or one
thing that several others point at: the nodes of a graph, an object in a
scene, a shared configuration. `Ref(value)` gives a value an identity:

```tondo
import std.console

type Module = { name: String, imports: Array[Ref[Module]] }

fn main() {
    let util = Ref(Module { name: "util", imports: [] })
    let http = Ref(Module { name: "http", imports: [util] })
    let json = Ref(Module { name: "json", imports: [util] })

    // `util` is one module with two importers, not two equal copies.
    console.println("{http.value.imports[0] == json.value.imports[0]}")

    // Another module that happens to be equal is a different one.
    let twin = Ref(Module { name: "util", imports: [] })
    console.println("{util == twin} {util.value.name == twin.value.name}")

    var seen: Set[Ref[Module]] = Set[util]
    _ = seen.insert(http.value.imports[0])
    console.println("{seen.length()} {http.value.name} imports {http.value.imports.length()}")
}
```

```text output
true
false true
1 http imports 1
```

- Copying a `Ref` copies the reference: all the copies are the same object.
- `==` and hashing compare identities, so a `Ref` is a key of a map or an
  element of a set whatever it holds.
- `reference.value` reads what it holds, and what it holds never changes. A
  `Ref` gives identity, not mutability.
- What does change goes in something made for that, inside the `Ref`: a
  `sync.Mutex`, a `sync.Atomic`, a shared collection.

```tondo
import std.console
import std.sync

type Person = { name: String, friends: sync.Mutex[Array[Ref[Person]]] }

fn person(name: String): Ref[Person] {
    Ref(Person { name, friends: sync.Mutex([]) })
}

fn befriend(a: Ref[Person], b: Ref[Person]) {
    a.value.friends.update((list) { list.push(b) })
    b.value.friends.update((list) { list.push(a) })
}

fn main() {
    let ada = person("ada")
    let bo = person("bo")
    befriend(ada, bo)
    let friendOfFriend = ada.value.friends.get()[0].value.friends.get()[0]
    console.println("{friendOfFriend.value.name} {friendOfFriend == ada}")
}
```

```text output
ada true
```

`ada` and `bo` now refer to each other. When nothing else refers to either,
both are freed: a cycle needs nothing broken by hand.

## WeakRef

`WeakRef(reference)` refers to the same object without keeping it alive.
`get()` gives the `Ref` while the object lives, and `none` after. It is for
a cache, or a table of things that others own:

```tondo
import std.console
import std.sync

type Image = { name: String, pixels: Array[Int] }

fn load(cache: sync.Mutex[Map[String, WeakRef[Image]]], name: String): Ref[Image] {
    match cache.get().get(name).andThen((weak) { weak.get() }) {
        some(image) => image
        none => {
            let image = Ref(Image { name, pixels: [1, 2, 3] })
            cache.update((entries) { entries[name] = WeakRef(image) })
            image
        }
    }
}

fn main() {
    let cache: sync.Mutex[Map[String, WeakRef[Image]]] = sync.Mutex([:])
    let first = load(cache, "logo")
    let again = load(cache, "logo")
    console.println("{first == again} {first.value.pixels.length()}")

    let temporary = WeakRef(Ref(Image { name: "tmp", pixels: [] }))
    console.println("{temporary.get().isNone()}")
}
```

```text output
true 3
true
```

A program never needs a `WeakRef` to avoid a leak. It needs one only when it
wants to look at an object without being a reason for it to stay.

## How memory is managed

Memory is counted. Each string, collection and `Ref` knows how many hold
it, and is freed at once when the last one lets go: when a variable goes out
of scope, is assigned again, or its owner is freed. Nothing waits for a
collector to come by, so a program's memory follows what it holds.

Counting alone would never free two objects that hold each other. A
collector of cycles finds those. A cycle can only close through state that
changes after it is made, such as a `Mutex` or a channel, since values,
closures and `Ref`s do not change once built. The collector starts from
those places, and stops the tasks only briefly, at points where it is safe,
when there is enough new garbage of that kind to look for.

What this means in practice:

- Nothing to free, and no way to use something after it is freed.
- No finalizers: no code of the program runs at a moment it did not choose.
- Most values never reach the heap. A value that does not outlive its
  function stays in the function's frame, and the compiler puts strings,
  closures and records there when it can prove that. `tondo optimizations
  --kind heap` lists what allocates, and why.
- `tondo profile --memory` shows where a running program allocates and what
  it holds, and a dump shows the live memory by the line that made it.

## Resources outside the program

Memory is managed. A file, a socket, a process or a task is not memory: each
is something outside the value that holds it, and each is released when its
value goes away, at the end of the block that owns it. A file is flushed and
closed, a task is waited for, a lock is let go.

```tondo
import std.console
import std.fs

fn firstLine(path: String): String? ! fs.FsError {
    var file = fs.open(path, fs.OpenMode.Read)?
    let line = file.readLine()?
    file.close()?               // by hand, to see its error; the end of the block would close it too
    line
}

fn main(): !fs.FsError {
    let directory = fs.createTemporaryDirectory()?
    defer {
        _ = fs.remove(directory, recursive: true)
    }
    let path = "{directory}/notes.txt"
    fs.writeText(path, "one\ntwo\n")?
    console.println("{firstLine(path)?} {fs.readText(path)?.lines().length()}")
}
```

```text output
some(one) 2
```

- A release that happens by itself cannot report a failure. When the
  failure matters, as for a file that was written to, call `close()` and
  handle what it gives.
- These values are among the few that [cannot be
  copied](#values-that-cannot-be-copied): a file has one owner, and
  `close()` takes it.
- `defer` is for cleanup that no value stands for: removing a temporary
  directory, stopping a server, writing a log line. It runs however the
  block ends.
- Most code never opens anything by hand, since `fs.readText`, `http.get`
  and their kin open, use and close in one call.

## Sharing between tasks

Values cross to another task by being copied or moved there, never lent. The
compiler checks it: what goes to a task must be `Send`, and what several
tasks read at once must be `Share`, both of which a type has when its parts
do. A plain value has them. So there is no data race in safe code, and
nothing to annotate to get that.

State that several tasks change together goes in `std.sync`
([Tasks](13-tasks.md)): its types are the ones built to be shared, and the
only ones that are.

## Which to use

| You want | Use |
| --- | --- |
| to pass data around | a value: it is copied, cheaply |
| a function to change the caller's value | a `mut` or `var` parameter |
| to read something large in a hot loop | a `ref` parameter, or `for ref` |
| one thing that several others point at | `Ref[T]` |
| to look at an object without keeping it | `WeakRef[T]` |
| state that changes, shared by tasks | `sync.Mutex`, `sync.Atomic`, a shared collection |
| to hand work to another task | a channel |
| memory by hand, for C | `Pointer[T]`, inside `unsafe` ([Unsafe and C](18-unsafe.md)) |

Start with values. The rest are for the cases the table names.

Next: [Tasks](13-tasks.md).
