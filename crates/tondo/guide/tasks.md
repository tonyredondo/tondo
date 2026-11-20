# Tasks

Any function can wait: there is no `async`/`await`. `spawn call()` starts a
task and gives a `Join`; waiting stops the task, not the thread, and tasks run
on all cores.

```tondo
import std.console
import std.time

fn fetch(id: Int): Int {
    time.sleep(time.milliseconds(10))
    id * 10
}

fn main() {
    let job = spawn fetch(1)
    console.println("{job.wait()}")

    // An array of tasks: one for each element, every result in its order.
    let pages = [1, 2, 3].map((id) { spawn fetch(id) }).wait()
    console.println("{pages}")

    // A group: many tasks, results in order of adding.
    var group = Group[Int]()
    for id in 0..5 {
        group.add(spawn fetch(id))
    }
    console.println("{group.wait()}")

    // Fire and forget.
    spawn detached fetch(9)
}
```

- A `Join` nobody waits for is waited at the end of its block, so no task is
  left behind by accident. In a loop that means one task at a time, which
  warns (`W1013`): `spawn detached` launches a task nobody waits for, as a
  server does for each connection, and a loop that gives its tasks,
  `let jobs = for x in xs { spawn f(x) }`, has them all to wait for at once.
- An array of tasks is waited for whole: `ids.map((id) { spawn fetch(id) })`
  starts one for each element, and `wait()` on it gives every result in the
  array's order and cancels nothing, each keeping its own error; `cancel()`
  tells them all to stop and waits. Each uses the array up (`E1401` after).
- `waitOrCancel()`, on an array of tasks or a `Group` whose tasks can fail,
  gives `Array[T] ! E`: all the values, or the first error to happen, which
  cancels the others. `ids.map((id) { spawn load(id) }).waitOrCancel()?`
  passes that error on.
- Tasks of different types go in a tuple, waited for the same three ways:
  `let (page, user) = (spawn fetch(url), spawn load(id)).waitOrCancel()?`
  gives the values or the first error, which fails with the union of the
  tasks' errors, `! (HttpError | DbError)`; a task that cannot fail may sit
  beside those that can. `wait()` gives each result with its own type.
- `within tasks.limit(64) { ... }` (`import std.tasks`) bounds how many of
  the tasks a block starts are alive at once: a `spawn` in it waits for a
  place before its task exists. The tasks those tasks start are not counted,
  `spawn detached` takes no place, and a limit kept in a value is shared by
  every `within` of it. With a deadline:
  `within time.seconds(5), tasks.limit(64) { ... }`.
- A `Group` is for tasks added as the work goes and results taken as they
  finish: `group.next()`, or `for value in group { ... }`, which leaves the
  tasks still running in the group after a `break`.
- `job.cancel()` stops a task at its next wait and runs its `defer`s, which
  still do what needs no wait, as taking a free lock or sending to a channel
  with room, so shared state stays right. A wait a `defer` needs ends at
  once, and ends that `defer`.
- A loop that computes without waiting is not interrupted, by a cancel or by
  the deadline of a `within`: call `tasks.checkpoint()` in it
  (`import std.tasks`), which stops the task there as a wait would and
  otherwise does nothing. In a loop of a few instructions a turn, call it
  every thousand turns: `if n % 1024 == 0 { tasks.checkpoint() }`.
  `tasks.cancelled()` asks without stopping, for a `defer` or to end with
  what there is.
- A checkpoint also lets the others run: when every worker is busy and tasks
  wait, the task that asks gives its worker away after about 10 ms on it and
  goes on at its next turn, so a loop that asks holds nobody up, with one
  worker either, and another task's cancel reaches it. Code that never asks
  gets a worker put beside it after about a tenth of a second, and cannot be
  cancelled before it ends: `workers added` in `tondo counters` counts those,
  and a loop that shows there wants a checkpoint.
- A panic in a task ends the program, unless the task was started in a
  `recover { ... }`, without `detached`: its panic stops that block, which
  cancels its other tasks (see `tondo guide errors`).

A task that can fail is not dropped where it is made: nothing would read
its failure. Keep its `Join` and wait for it, `job.wait()?`, or handle the
failure in the task; `spawn detached` of such a call needs `_ =` before it,
to say the failure is dropped on purpose. `_ = spawn f()` is no way out: it
drops the `Join` at once, so it waits for the task right there (`W1026`).

```tondo error E1303
type Order = {
    id: Int
}

fn save(order: Order): !String {
    if order.id == 2 {
        fail "order {order.id} could not be saved"
    }
}

fn main() {
    spawn save(Order { id: 2 })
}
```

`wait()` on tasks that can fail gives every outcome, each with its own
error, and cancels none: an array of results, not one result. For the
values or the first error, which cancels the rest, write `waitOrCancel()`.

```tondo error E1621
fn load(id: Int): Int ! String {
    if id == 2 {
        fail "item {id} is missing"
    }
    id
}

fn loadAll(ids: Array[Int]): Array[Int] ! String {
    let loaded = ids.map((id) { spawn load(id) }).wait()?
    loaded
}

fn main() {
    _ = loadAll([1, 2, 3])
}
```

`waitOrCancel()` stops at the first error, so it is for tasks that can
fail. On tasks that cannot, write `wait()`.

```tondo error E1620
fn double(n: Int): Int {
    n * 2
}

fn main() {
    let doubled = [1, 2, 3].map((n) { spawn double(n) }).waitOrCancel()
    _ = doubled
}
```

## Channels and select

```tondo
import std.console
import std.time

fn produce(out: Sender[Int]) {
    for n in 0..3 {
        out.send(n)
    }
}

fn main() {
    let (sender, receiver) = channel[Int](8)
    spawn produce(sender)
    for value in receiver {                     // until closed and empty
        console.println("{value}")
    }

    let (requests, inbox) = channel[String](1)
    let slow = spawn produce2()
    let event = select {
        let message = inbox.receive() => "message {message}"
        let n = slow.wait() => "done {n}"
        time.sleep(time.milliseconds(5)) => "timeout"
    }
    console.println(event)
    requests.close()
}

fn produce2(): Int {
    time.sleep(time.milliseconds(50))
    1
}
```

A `Sender` moves (the channel closes when its last sender goes); a
`Receiver` can be shared, and only senders close. `receive(timeout: d)`
gives `none` on time out. `sender.isAbandoned()` says no receiver is left,
for a producer to stop, and `channel[T](n, discardWhenAbandoned: true)`
makes `send` drop values nobody can receive instead of keeping or waiting.
`select` waits for the first ready arm; `else =>` makes it not wait.

## Shared state

`std.sync` has `Mutex`, `RwLock`, `Atomic`, `Semaphore`, `Once`, `Barrier`
and shared collections (`sync.Map`, `sync.Queue`…); `tondo api std.sync`.
A lock is let go when its closure returns, is cancelled or panics: a panic
that a `recover` stops leaves it free, never poisoned.

```tondo
import std.console
import std.sync

pub type Stats = { hits: Int }

fn main() {
    let stats = sync.Mutex(Stats { hits: 0 })
    let counter = sync.Atomic(0)
    let recording = for _ in 0..4 { spawn record(stats, counter) }
    recording.wait()
    console.println("{stats.get().hits} {counter.load()}")
}

fn record(stats: sync.Mutex[Stats], counter: sync.Atomic[Int]) {
    stats.update((s) { s.hits += 1 })
    _ = counter.add(1)
}
```

`sync.RateLimiter(100, per: time.seconds(1))` paces work: `allow()` says
whether one more may go now, `wait()` waits for its turn, and its copies
share it. `sync.retry(work, attempts: 5, when: (e) { e.isTransient() })`
tries failing work again, waiting a random, doubling delay between tries.

## Pools and actors

`std.executor` bounds work: a pool runs at most `workers` jobs at a time
and keeps at most `capacity` waiting, so `submit` waits while it is full
(`trySubmit` fails instead). A job is an ordinary task: its `Join` is
waited for and cancelled as any. An actor owns state that one task changes,
a message at a time; `reference()` gives the handle to send to it, and the
end of the block that owns the actor stops it after its mailbox.

```tondo
import std.console
import std.executor

fn main(): !(executor.ExecutorError | executor.SubmitError | executor.SendError) {
    let pool = executor.pool(2, 8)?
    let squares = for n in 1..=5 { pool.submit(() { n * n })? }
    console.println("{squares.wait()}")
    pool.shutdown()

    let counter = executor.actor(0, 16, (total: var Int, amount: Int) {
        total += amount
        console.println("total {total}")
    })?
    let address = counter.reference()
    address.send(2)?
    address.send(3)?
    counter.stop()?
}
```

## Contexts

A value that belongs to the work that runs, as the request, the user or the
trace, is a `context`: any function reads it without a parameter.

```tondo
import std.console

context Request = {
    id: String = ""
    user: String? = none
}

fn greet(): String {
    let user = Request.user ?? "guest"
    "hello {user} ({Request.id})"
}

fn handle(id: String, user: String) {
    with Request { id, user: user }
    console.println(greet())
}

fn main() {
    handle("r1", "ada")
    console.println(greet())
}
```

- `with Request { field: value }` changes it from there to the end of the
  block, whichever way the block ends; several fields go in one `with`, and
  several contexts in lines one after another. A change never reaches the
  caller.
- A task takes the context of the point where it is spawned. A value set
  at the start of `main` is the whole program's, as a tracer or a
  configuration: there are no mutable globals.
- `Contexts.current()` captures every context as it is there, a `Copy` and
  `Send` value; `with captured` gives them back to the end of the block, in
  this task or another. A queue keeps them with each message, and its worker
  handles each with its sender's. A function cannot change its caller's
  contexts, but it can return the ones it would have, which the caller
  activates: `with tracer.startSpan("db.query")`.
- An initial value that is not a constant, as a compiled pattern, a pool
  or a `sync.Map`, is computed once, when the program first reads it, and
  every task then shares it: `words: Regex = Regex.compile(r"\w+")?`. This
  is where a program's or a library's shared values go. A `?` that fails
  there panics at that read, and the next one tries again. Fields are
  `Send` and `Copy`.
- A field marked `@log("request_id")` goes in every event of `std.log` and
  in a panic's report while it is set; one marked `@label` is a label of
  profiles. `log.Context` holds the logger that `log.info(...)` and the other
  functions of `std.log` write with.

`within limit { ... }` gives a block a deadline: when it passes, the
block's next wait ends, its `defer`s run and its tasks are cancelled, and
the result is `err(time.TimedOut)`. In the `defer`s of a cancelled task,
as on SIGTERM, a wait ends at once, but one inside a `within` goes on until
its deadline: a library's `stop()` that sends what it has left wraps the
send in `within time.seconds(5) { ... }`, and `defer tracer.stop()` is all
its caller writes.

```tondo
import std.console
import std.time

fn fetch(): String {
    time.sleep(time.seconds(10))
    "page"
}

fn main() {
    match within time.milliseconds(100) { fetch() } {
        ok(page) => console.println(page)
        err(timedOut) => console.println("{timedOut}")
    }
}
```

- A block of type `T` gives `T ! time.TimedOut`; one of `T ! E`, `T ! (E |
  time.TimedOut)`, which `?` passes on. A computation that does not wait is
  not stopped.
- Tasks the block starts see the deadline (`time.deadline()`,
  `time.remaining()`), and a detached task does not. A `recover` lets a
  deadline go through, and a `within` lets a panic go through.
