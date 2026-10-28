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

    // A group: many tasks, results in order of adding.
    var group = Group.new[Int]()
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
  server does for each connection, and a `Group` waits for many at once.
- With `Group[T ! E]`, `wait()` gives `Array[T] ! E`: the first error cancels
  the others.
- A panic in a task ends the program, unless the task was started in a
  `recover { ... }`, without `detached`: its panic stops that block, which
  cancels its other tasks (see `tondo guide errors`).

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
`Receiver` can be shared. `receive(timeout: d)` gives `none` on time out.
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
    var group = Group.new[Unit]()
    for _ in 0..4 {
        group.add(spawn record(stats, counter))
    }
    group.wait()
    console.println("{stats.get().hits} {counter.load()}")
}

fn record(stats: sync.Mutex[Stats], counter: sync.Atomic[Int]) {
    stats.update((s) { s.hits += 1 })
    _ = counter.add(1)
}
```

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
    var group = Group.new[Int]()
    for n in 1..=5 {
        group.add(pool.submit(() { n * n })?)
    }
    console.println("{group.wait()}")
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
    let user = Request.user.unwrapOr("guest")
    "hello {user} ({Request.id})"
}

fn handle(id: String, user: String) {
    with Request { id, user: some(user) }
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
- Initial values are constants; a value made as the program runs is a
  `T? = none`, set with `with`. Fields are `Send` and `Copy`.
