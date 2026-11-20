# Tasks

A task is a function running at the same time as others. Tasks are cheap,
they run on all the machine's cores, and any function may wait inside one
without being marked: there is no `async` and no `await`. This chapter
covers starting tasks and getting their results, channels, `select`,
cancelling, sharing state, and pools and actors.

- [Any function can wait](#any-function-can-wait)
- [spawn and Join](#spawn-and-join)
- [A task belongs to its block](#a-task-belongs-to-its-block)
- [Groups](#groups)
- [Errors and panics in tasks](#errors-and-panics-in-tasks)
- [Cancelling](#cancelling)
- [Channels](#channels)
- [select](#select)
- [Signals](#signals)
- [Shared state](#shared-state)
- [Atomics, and updates that get lost](#atomics-and-updates-that-get-lost)
- [Shared collections](#shared-collections)
- [The other tools of std.sync](#the-other-tools-of-stdsync)
- [Pools and actors](#pools-and-actors)
- [Deadlocks](#deadlocks)
- [Tasks and threads](#tasks-and-threads)
- [Choosing](#choosing)

## Any function can wait

Reading a file, asking a server, sleeping, taking a lock: each may have to
wait. In Tondo that is not part of a function's type. A function that waits
is declared and called like one that does not:

```tondo
import std.console
import std.time

fn fetch(id: Int): String {
    time.sleep(time.milliseconds(20))       // stands for a request
    "page {id}"
}

fn main() {
    let page = fetch(1)
    console.println(page)
}
```

```text output
page 1
```

While a task waits, its thread runs another task. So there is no second kind
of function, no second version of each library, and nothing that changes in
the callers of a function the day it starts to wait.

## spawn and Join

`spawn call(...)` starts the call as a task and gives a `Join`, the handle to
its result. `wait()` gives the result, waiting for it if it is not ready:

```tondo
import std.console
import std.time

fn fetch(id: Int): String {
    time.sleep(time.milliseconds(20))
    "page {id}"
}

fn main() {
    let started = time.now()
    let first = spawn fetch(1)
    let second = spawn fetch(2)             // both are running now
    let pages = [first.wait(), second.wait()]
    console.println("{pages} in {started.elapsed().milliseconds()} ms")
}
```

It prints the two pages after about 20 milliseconds, not 40: the two calls
waited at the same time.

`spawn` takes a call: a function, a method or a closure, with its arguments.
The arguments are evaluated at the `spawn` and given to the task by value,
so the task has its own and shares nothing with the code that started it
unless what it was given is made to be shared.

```tondo
import std.console

fn main() {
    let name = "worker"
    let job = spawn (() {
        "{name} done"
    })()
    console.println(job.wait())
}
```

```text output
worker done
```

## A task belongs to its block

A `Join` that nobody waited for is waited for when its block ends. A task is
never left running by accident, with its errors lost and its work
half-done when the program exits:

```tondo
import std.console
import std.sync
import std.time

fn save(done: sync.Atomic[Int], id: Int) {
    time.sleep(time.milliseconds(10))
    _ = done.add(1)
}

fn saveBoth(done: sync.Atomic[Int]) {
    let first = spawn save(done, 1)
    let second = spawn save(done, 2)
    _ = first
    _ = second
}                                           // both are waited for here

fn main() {
    let done = sync.Atomic(0)
    saveBoth(done)
    console.println("{done.load()}")
}
```

```text output
2
```

This is structured concurrency: the tasks a block starts end before the
block does, as the calls it makes do. A function that returns has left
nothing behind.

A task that should go on after its block says so, with `spawn detached`. It
is for the task nobody will wait for, as the one a server starts for each
connection:

```tondo
import std.console
import std.time

fn heartbeat() {
    for {
        time.sleep(time.seconds(1))
        console.println("alive")
    }
}

fn main() {
    spawn detached heartbeat()
    console.println("started")
}
```

A program ends when `main` does, and detached tasks are cancelled then.
`job.detach()` does the same to a task that was started with a `Join`.

A task that can fail is a result that somebody has to read. Started and
left alone, it would be waited for at the end of its block and its failure
would go nowhere, so the compiler refuses it where it is made:

```text
error[E1303]: the task of `save` may fail, and nothing will read its failure: keep its `Join` and wait for it, as `let job = spawn save(Order { id: 2 })` and later `job.wait()?`, or handle the failure in the task
```

Either the code that starts the task waits for it where the work has to be
done, and gets its failure there, or the task takes care of its own:

```tondo
import std.console

type Order = {
    id: Int
}

fn save(order: Order): !String {
    if order.id == 2 {
        fail "order {order.id} could not be saved"
    }
}

/// Saves beside other work, and passes a failure on.
fn place(order: Order): !String {
    let saving = spawn save(order)
    console.println("order {order.id}: packing")
    saving.wait()?
}

/// Saves, and says so itself when it cannot.
fn saveOrSay(order: Order) {
    match save(order) {
        ok(_) => {}
        err(why) => console.println("not saved: {why}")
    }
}

fn main() {
    for id in [1, 2] {
        match place(Order { id }) {
            ok(_) => console.println("order {id}: placed")
            err(why) => console.println("order {id}: {why}")
        }
    }
    spawn saveOrSay(Order { id: 2 })
}
```

```text output
order 1: packing
order 1: placed
order 2: packing
order 2: order 2 could not be saved
not saved: order 2 could not be saved
```

`_ = saving.wait()` drops a failure on purpose, at the place where the
work is done. `_ = spawn save(order)` does not: it lets go of the `Join` at
once, so the task is waited for right there, as a call would be, and the
compiler warns (`W1026`). A `spawn detached` of a call that can fail has
nobody to wait for it, and is written `_ = spawn detached save(order)` to
say that its failure is dropped, as a server does with each connection.

One consequence: a `spawn` in a loop whose `Join` is dropped waits for each
task before starting the next, which is rarely what was meant. The compiler
warns (`W1013`) and says what to write instead: an array of tasks, a
`Group`, or `detached`. To start a task for each value and wait for them
all, the tasks go in an array.

## An array of tasks

`map` gives an array of what its closure gives, and a closure that spawns
gives a task. So one line starts a task for each element, and `wait()` on
the array waits for them all and gives their results, in the array's order
whichever finished first:

```tondo
import std.console
import std.time

fn fetch(id: Int): String {
    time.sleep(time.milliseconds(10 * (4 - id)))  // the last one finishes first
    "page {id}"
}

fn main() {
    let pages = [1, 2, 3].map((id) { spawn fetch(id) }).wait()
    console.println("{pages}")
}
```

```text output
[page 1, page 2, page 3]
```

`wait()` cancels nothing. When the tasks can fail, each result keeps its
own error, and a task that failed does not stop the others:

```tondo
import std.console
import std.time

fn load(id: Int): String ! String {
    time.sleep(time.milliseconds(5 * id))
    if id == 2 {
        fail "item {id} is missing"
    }
    "item {id}"
}

fn main() {
    let loaded = [1, 2, 3].map((id) { spawn load(id) }).wait()
    console.println("{loaded}")
}
```

```text output
[ok(item 1), err(item 2 is missing), ok(item 3)]
```

It is the same as waiting for each task in turn, `first.wait()` and then
the next, so a task that panicked does what its own `wait()` would. An
array written out is waited for the same way, as `[spawn one(), spawn
two()].wait()`.

`waitOrCancel()` is for when one failure makes the rest pointless. It gives
all the values, or the first error to happen, and that error cancels the
tasks still running:

```tondo
import std.console
import std.time

fn load(id: Int): String ! String {
    time.sleep(time.milliseconds(5 * id))
    if id == 2 {
        fail "item {id} is missing"
    }
    "item {id}"
}

fn loadAll(ids: Array[Int]): Array[String] ! String {
    ids.map((id) { spawn load(id) }).waitOrCancel()
}

fn main() {
    console.println("{loadAll([1, 3])}")
    console.println("{loadAll([1, 2, 3])}")
}
```

```text output
ok([item 1, item 3])
err(item 2 is missing)
```

With `?` after it the error goes on to the caller as any other does. On
tasks that cannot fail there is nothing to stop at, and the compiler says
to write `wait()` (`E1620`).

`cancel()` on the array tells every task to stop, and then waits until all
of them have. Each of the three uses the array up, as `wait()` does a
`Join`: the compiler refuses a use of it afterwards (`E1401`).

## Groups

An array is the short way when the tasks are all there at the start. A
`Group` is for the rest: tasks that are added as the work finds them, and
results that are taken as they finish. It waits for its tasks together, as
an array does, and its results come in the order the tasks were added,
whichever finished first:

```tondo
import std.console
import std.time

fn square(n: Int): Int {
    time.sleep(time.milliseconds(10 * (5 - n)))     // the last one finishes first
    n * n
}

fn main() {
    var group = Group[Int]()
    for n in 1..=4 {
        group.add(spawn square(n))
    }
    console.println("{group.wait()}")
}
```

```text output
[1, 4, 9, 16]
```

`group.next()` gives the results one at a time, as they finish, for work
that should be handled as it comes:

```tondo
import std.console
import std.time

fn check(host: String, delay: Int): String {
    time.sleep(time.milliseconds(delay))
    host
}

fn main() {
    var group = Group[String]()
    group.add(spawn check("slow.example", 300))
    group.add(spawn check("fast.example", 1))
    let first = group.next()
    console.println("{first} answered first, {group.length()} still running")
    group.cancel()
}
```

```text output
some(fast.example) answered first, 1 still running
```

A `for` over the group takes each result as its task finishes, until none
is left. It does not use the group up: after a `break` the tasks still
running are in it, to be waited for or cancelled, and more can be added:

```tondo
import std.console
import std.time

fn check(host: String, delay: Int): String {
    time.sleep(time.milliseconds(delay))
    host
}

fn main() {
    var group = Group[String]()
    group.add(spawn check("slow.example", 300))
    group.add(spawn check("fast.example", 1))
    group.add(spawn check("mid.example", 50))
    for host in group {
        console.println("{host} answered")
        if host == "mid.example" {
            break
        }
    }
    console.println("{group.length()} still running")
    group.cancel()
}
```

```text output
fast.example answered
mid.example answered
1 still running
```

A group has the three ways of an array to end: `wait()` for every result,
`waitOrCancel()` for the values or the first error, and `cancel()`.

## Errors and panics in tasks

A task's result is whatever its function returns, errors included. On an
array of tasks and on a `Group` alike, `wait()` gives every outcome with
its own error, and `waitOrCancel()` gives all the values or the first
error ([An array of tasks](#an-array-of-tasks)).

What `wait()` gives on tasks that can fail is an array of results, not one
result. Using it as one is a compile error that says what was meant:

```text
error[E1621]: `wait()` gives every task's result, as `Array[Int ! String]`, and cancels none; for the values or the first error, which cancels the other tasks, write `waitOrCancel()`
```

And an array of results that nobody reads is refused as a result that
nobody reads is, since any of its values may be a failure:

```text
error[E1303]: a value of type `Array[Int ! String]` is not used, and it holds results that may be failures; read them, discard them with `_ = ...`, or pass the first failure on with `waitOrCancel()?`
```

A panic in a task is a bug and ends the program, with a report that shows
the task's stack and where it was spawned. Inside a `recover { ... }`, a
panic in a task the block started stops that block instead, and its other
tasks are cancelled ([Errors](07-errors.md)).

## Cancelling

`job.cancel()` asks a task to stop and waits until it has. The task stops at
its next wait, and its `defer`s run:

```tondo
import std.console
import std.sync
import std.time

fn work(cleaned: sync.Atomic[Bool]) {
    defer {
        cleaned.store(true)
    }
    for {
        time.sleep(time.milliseconds(5))        // a wait: where a cancel lands
    }
}

fn main() {
    let cleaned = sync.Atomic(false)
    let job = spawn work(cleaned)
    time.sleep(time.milliseconds(20))
    job.cancel()
    console.println("cancelled, cleaned up: {cleaned.load()}")
}
```

```text output
cancelled, cleaned up: true
```

- Cancelling is cooperative. A task that only computes is not interrupted:
  it is cancelled when it next waits.
- Cancelling a task cancels the tasks it started and still owns.
- In the `defer`s of a cancelled task, what needs no waiting still works,
  as taking a free lock or sending to a channel with room, so shared state
  is left right. A wait there ends at once, unless it is inside a
  `within`, which gives it until its deadline ([Contexts and
  deadlines](14-contexts.md)).
- Ctrl-C and `SIGTERM` cancel `main`, so a program's own cleanup is the
  same code whether it ends by itself or is told to. A program that wants
  to decide what they do takes them as [signals](#signals).

## Channels

A channel carries values from one task to another. `channel[T](capacity)`
gives its two ends:

```tondo
import std.console

fn produce(out: Sender[Int], count: Int) {
    for n in 1..=count {
        out.send(n * n)
    }
}                                               // the last sender goes: the channel closes

fn main() {
    let (sender, receiver) = channel[Int](8)
    spawn produce(sender, 5)
    var total = 0
    for value in receiver {                     // until closed and empty
        total += value
    }
    console.println("{total}")
}
```

```text output
55
```

- With a capacity, `send` waits while the channel is full, which keeps a
  fast producer from running ahead of a slow consumer. Without one,
  `channel[T]()`, the channel has no limit and `send` never waits.
- A `Sender` has one owner: passing it moves it. `sender.copy()` makes
  another for a second producer. The channel closes when its last sender
  goes away, or with `close()`.
- A `Receiver` can be copied, and several tasks can receive from one
  channel: each value goes to one of them.
- `receive()` gives the next value, or `none` once the channel is closed
  and empty. `receive(timeout: d)` also gives `none` when the time passes.
- Sending on a closed channel is a panic. A producer whose consumers have
  all gone finds out with `sender.isAbandoned()`.

Several producers and several consumers:

```tondo
import std.console

fn produce(out: Sender[Int], start: Int) {
    for n in start..start + 3 {
        out.send(n)
    }
}

fn consume(jobs: Receiver[Int], results: Sender[Int]) {
    for job in jobs {
        results.send(job * 10)
    }
}

fn main() {
    let (jobs, inbox) = channel[Int](4)
    let (results, outbox) = channel[Int]()
    spawn produce(jobs.copy(), 0)
    spawn produce(jobs, 100)                    // the original moves here
    spawn consume(inbox, results.copy())
    spawn consume(inbox, results)
    var collected: Array[Int] = []
    for result in outbox {
        collected.push(result)
    }
    console.println("{collected.sorted()}")
}
```

```text output
[0, 10, 20, 1000, 1010, 1020]
```

## select

`select` waits for the first of several things to be ready, and runs its
arm:

```tondo
import std.console
import std.time

fn slow(): Int {
    time.sleep(time.milliseconds(50))
    42
}

fn main() {
    let (sender, inbox) = channel[String](1)
    let job = spawn slow()

    let first = select {
        let message = inbox.receive() => "a message: {message}"
        time.sleep(time.milliseconds(5)) => "nothing yet"
    }
    console.println(first)

    sender.send("hello")
    let second = select {
        let message = inbox.receive() => "a message: {message}"
        time.sleep(time.seconds(5)) => "nothing yet"
    }
    console.println(second)

    let third = select {
        let answer = job.wait() => "the answer: {answer}"
        time.sleep(time.seconds(5)) => "too slow"
    }
    console.println(third)
}
```

```text output
nothing yet
a message: some(hello)
the answer: 42
```

An arm is a receive, a send, the wait of a `Join`, or a sleep, which makes a
timeout. `select` is an expression: its arms give a value. With `else =>`,
it does not wait at all and takes that arm when nothing is ready:

```tondo
import std.console

fn main() {
    let (sender, inbox) = channel[Int](1)
    let peek = select {
        let value = inbox.receive() => "got {value}"
        else => "empty"
    }
    sender.send(7)
    let again = select {
        let value = inbox.receive() => "got {value}"
        else => "empty"
    }
    console.println("{peek}, {again}")
}
```

```text output
empty, got some(7)
```

## Signals

A program that does nothing about them is cancelled by Ctrl-C, which is
`Interrupt`, and by `Terminate`: its tasks run their `defer`s, it leaves
with 130 or 143, and a second one ends it at once. `process.signals` takes
the signals it names out of that rule and gives them on a channel, for the
program to decide what each does:

```tondo
import std.console
import std.env
import std.process

fn main() {
    let signals = process.signals(process.Signal.Hangup, process.Signal.Interrupt)
    // What a terminal or a supervisor would send, sent here by the program itself.
    _ = process.signal(env.processId(), process.Signal.Hangup)
    _ = process.signal(env.processId(), process.Signal.Interrupt)
    for signal in signals {
        match signal {
            Hangup => console.println("reloading the settings")
            _ => {
                console.println("closing what is open, and leaving")
                break
            }
        }
    }
}
```

```text output
reloading the settings
closing what is open, and leaving
```

- The signals are `Interrupt`, `Terminate`, `Hangup`, `User1`, `User2` and
  `Quit`. `Kill` can be sent and never received.
- Two equal ones that the channel has not given yet are one.
- The channel is one more thing to wait for in a `select`, beside the
  program's work.
- `process.signal(pid, signal)` sends one to any process, and the `signal`
  of a process the program started sends it to that one
  ([`std.process`](../std/process.md)).
- A program that writes to a pipe whose reader has gone, as `| head`, ends
  without a word, as a command should.

## Shared state

Tasks that must work on the same data share it through `std.sync`. A
`Mutex` holds a value that one task at a time may change:

```tondo
import std.console
import std.sync

type Stats = { requests: Int, errors: Int, slowest: Int }

fn record(stats: sync.Mutex[Stats], took: Int, failed: Bool) {
    stats.update((current) {
        current.requests += 1
        if failed {
            current.errors += 1
        }
        current.slowest = current.slowest.max(took)
    })
}

fn main() {
    let stats = sync.Mutex(Stats { requests: 0, errors: 0, slowest: 0 })
    var group = Group[Unit]()
    for n in 1..=100 {
        group.add(spawn record(stats, n, n % 10 == 0))
    }
    group.wait()
    let final = stats.get()
    console.println("{final.requests} requests, {final.errors} errors, slowest {final.slowest}")
}
```

```text output
100 requests, 10 errors, slowest 100
```

- `update` runs a closure with the value, alone. The lock is taken before
  and let go after, however the closure ends: a return, a cancel, a panic.
  There is no unlock to forget, and a lock is never left held or poisoned.
- `get()` gives a copy of the value, and `set(value)` replaces it.
- A `Mutex` is copied like a `Ref`: every copy is the same lock, which is
  why `stats` can be given to a hundred tasks.
- `updateWhen(ready, change)` waits until the value satisfies a condition
  and then changes it, as a consumer that waits for a queue to have
  something.
- `RwLock` is the same for a value read far more than written:
  `lock.read((value) { ... })` lets readers in together.

Safe code has no data races: the compiler lets a value cross to another task
only by copy or move, and only the types of `std.sync` are shared. Nothing
is shared that was not asked for.

## Atomics, and updates that get lost

`sync.Atomic` holds a number, a `Bool` or a `Char` that is read and written
in one step of the machine, with no lock:

```tondo
import std.console
import std.sync

fn main() {
    let hits = sync.Atomic(0)
    var group = Group[Int]()
    for _ in 0..1000 {
        group.add(spawn hits.add(1))
    }
    group.wait()
    console.println("{hits.load()} {hits.swap(0)} {hits.load()}")
}
```

```text output
1000 1000 0
```

No race does not mean no mistake. Reading a value, computing, and writing it
back is two steps, and another task can write in between:

```tondo
import std.sync

fn deposit(balance: sync.Mutex[Int], amount: Int) {
    let seen = balance.get()
    balance.set(seen + amount)          // another deposit may be lost here
}

fn depositSafely(balance: sync.Mutex[Int], amount: Int) {
    balance.update((value) { value += amount })
}

fn main() {
    let balance = sync.Mutex(0)
    deposit(balance, 10)
    depositSafely(balance, 5)
}
```

The compiler warns about the first (`W1016`), and `tondo test` runs a test
that starts tasks in every order of them that differs, so the order that
loses the deposit is found and reported with the command that repeats it
([Testing](16-testing.md)). The fix is always the same: read and write in
one `update`, or with `compareExchange` on an atomic.

## Shared collections

`std.sync` has collections that many tasks read and write at once without
one lock around the whole: `sync.Map`, `sync.Set`, `sync.Queue`,
`sync.Stack` and `sync.Array`.

```tondo
import std.console
import std.sync

fn count(words: Array[String], seen: sync.Set[String], lengths: sync.Map[Int, Int]) {
    for word in words {
        if seen.insert(word) {
            lengths.insert(word.length(), 1)
        }
    }
}

fn main() {
    let seen = sync.Set[String]()
    let lengths = sync.Map[Int, Int]()
    let first = spawn count(["to", "be", "or"], seen, lengths)
    let second = spawn count(["not", "to", "be"], seen, lengths)
    first.wait()
    second.wait()
    console.println("{seen.length()} words, lengths {lengths.snapshot().keys().collect().sorted()}")
}
```

```text output
4 words, lengths [2, 3]
```

Each operation is atomic by itself, and `snapshot()` gives an ordinary
collection of what there is at that moment. Use them when many tasks touch
one collection often; for state that changes as a whole, a `Mutex` around a
plain record is simpler and says more.

What is taken out of one of them is let go of as a local value is, before
the operation returns, when it holds something whose end shows: an end of a
channel, a task, a process, a file. A `Sender` taken out and dropped closes
its channel, if it was the last one:

```tondo
import std.console
import std.sync

fn main() {
    let waiting: sync.Map[Int, Sender[String]] = sync.Map()
    let receive = {
        let (send, receive) = channel[String](1)
        _ = waiting.insert(1, send)
        receive
    }
    _ = waiting.remove(1)
    console.println("{receive.receive()}")
}
```

```text output
none
```

A value that only holds memory, as a `String`, is freed a little later,
which a program cannot see: that is what lets a read take no lock.

## The other tools of std.sync

| Tool | For |
| --- | --- |
| `sync.Semaphore(n)` | at most `n` at once: `let permit = semaphore.acquire()`, then `permit.release()` |
| `sync.Once[T]()` | a value computed once, by whoever asks first: `once.getOrInit(() { ... })` |
| `sync.Barrier(n)` | `n` tasks that start a phase together |
| `sync.RateLimiter(100, per: time.seconds(1))` | pacing: `allow()` asks, `wait()` waits for a turn |
| `sync.retry(work, attempts: 5)` | failing work tried again, with a growing, random delay |

```tondo
import std.console
import std.sync
import std.time

fn main() {
    var attempts = sync.Atomic(0)
    let result = sync.retry((): String ! String {
        if attempts.add(1) < 3 {
            fail "busy"
        }
        "done on try {attempts.load()}"
    }, attempts: 5, delay: time.milliseconds(1))
    console.println("{result}")
}
```

```text output
ok(done on try 3)
```

[`std.sync`'s page](../std/sync.md) has each with its example.

## Pools and actors

`std.executor` bounds work. A pool runs at most so many jobs at a time and
keeps at most so many waiting, so a burst of work slows its producer instead
of filling memory:

```tondo
import std.console
import std.executor

fn main(): !(executor.ExecutorError | executor.SubmitError) {
    let pool = executor.pool(2, 8)?             // 2 at a time, 8 waiting
    var group = Group[Int]()
    for n in 1..=5 {
        group.add(pool.submit(() { n * n })?)
    }
    console.println("{group.wait()}")
    pool.shutdown()
}
```

```text output
[1, 4, 9, 16, 25]
```

A job of a pool is an ordinary task: `submit` gives its `Join`, which is
waited for and cancelled like any other.

An actor owns a piece of state that only its own task changes, one message
at a time. Other tasks hold its address and send to it:

```tondo
import std.console
import std.executor
import std.sync

fn main(): !(executor.ExecutorError | executor.SendError) {
    let seen = sync.Atomic(0)
    let counter = executor.actor(0, 16, (total: var Int, amount: Int) {
        total += amount
        seen.store(total)
    })?
    let address = counter.reference()
    address.send(2)?
    address.send(3)?
    counter.stop()?                             // after its mailbox is empty
    console.println("{seen.load()}")
}
```

```text output
5
```

## Deadlocks

Tasks that wait for each other for ever are a bug the runtime can see. When
every task waits and nothing can wake any of them, the program panics
(`P0013`) and lists where each one waits. A task that takes a lock it
already holds panics at once (`P0014`).

The cause is often not among the tasks that wait: the one that was to wake
them ended with an error, and nobody had read its result yet. The report
names it, with where it was spawned:

```tondo
import std.console
import std.fs

/// The report's lines, read by a task of its own.
fn load(path: String): Array[String] ! fs.FsError {
    fs.readText(path)?.lines()
}

fn main() {
    let (ready, started) = channel[Unit](1)
    let lines = spawn load("report.txt")
    // Waits for a word that only a load that went well would send.
    _ = started.receive()
    ready.send(())
    console.println("{lines.wait() ?? []}")
}
```

```text
panic[P0013]: deadlock: every task waits and none of them can wake the others
the main task waits at:
  main  report.to:13:9
task 2 ended with an error nobody read: err(fs.FsError.NotFound)
it was spawned at:
  main  report.to:11:17
```

A program that stands still without being a deadlock, as a server that
still listens, says the same in a dump taken while it runs: `tondo dump
<program>` and then `tondo dump read <dump> why`
([Debugging](../toolchain/debugging.md#a-failure-somewhere-else-dumps)).

A deadlock that has not happened yet can be found too: with
`tondo test --lock-order`, a test fails when its tasks take two locks in
both orders, even if this run got away with it. `tondo blocked <program>`
shows who waits for whom in a running program.

## Tasks and threads

Tasks are not threads. The runtime keeps one worker thread for each
processor the program may use and moves tasks among them; a task that waits
costs a little memory and no thread, so a server has a task for each
connection and may have a hundred thousand.

`env.workers()` is how many workers there are, and the number to size by
whatever a program shares out among tasks, as a pool or the pieces of a
job; `env.processors()` is the machine's. They are the same unless the
program was started with `TONDO_WORKERS=<n>` in its environment, which asks
for another number of workers, from 1 to 1024: one makes a program run its
tasks one at a time, and more than the processors brings out a race.

A task that calls into C, or computes for a long time without waiting, keeps
its worker busy. The runtime notices and starts another worker for the rest,
so one slow call does not hold the others up, and nothing has to be marked
as blocking.

## Choosing

| To | Use |
| --- | --- |
| do two things at once and use both results | `spawn` and `wait()` |
| do many things at once | an array of tasks and `wait()` |
| add tasks as the work goes, or take results as they finish | a `Group` |
| stop at the first failure | `waitOrCancel()` |
| bound how much runs at once | `executor.pool`, or a `Semaphore` |
| hand values from one task to another | a channel |
| wait for whichever comes first | `select` |
| give work a time limit | `within` |
| count | `sync.Atomic` |
| share state that changes | `sync.Mutex`, or `RwLock` when it is mostly read |
| share a collection many tasks write | `sync.Map`, `sync.Set`, `sync.Queue` |
| own state in one place and send it messages | `executor.actor` |

Next: [Contexts and deadlines](14-contexts.md).
