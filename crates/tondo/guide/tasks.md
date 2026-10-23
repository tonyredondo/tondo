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
    (spawn fetch(9)).detach()
}
```

- A `Join` nobody waits for is waited at the end of its block, so no task is
  left behind by accident; `detach()` lets one run on.
- With `Group[T ! E]`, `wait()` gives `Array[T] ! E`: the first error cancels
  the others.
- A panic in any task ends the program.

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
