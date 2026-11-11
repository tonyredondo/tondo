# std.executor

pools that bound how much work runs at once and how much waits, and actors that own state behind a mailbox. Both build on `spawn`, `Join` and channels, which stay the only tasks: a job is an ordinary task whose `Join` the caller owns, waits for and cancels as any other.

```tondo
import std.console
import std.executor

// Blocking work, as a C library's, runs on a bounded pool, not on the
// workers that run tasks.
let pool = executor.pool(2, 8)?
var squares = Group[Int]()
for n in 1..=4 {
    squares.add(pool.submit(() { n * n })?)
}
console.println("{squares.wait()}")
pool.shutdown()
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.executor` shows the same in a terminal.

- [Functions](#functions): [`pool`](#pool), [`actor`](#actor)
- [ExecutorError](#executorerror)
- [SubmitError](#submiterror)
- [SendError](#senderror)
- [Pool](#pool-1): [`Pool.submit`](#poolsubmit), [`Pool.trySubmit`](#pooltrysubmit), [`Pool.shutdown`](#poolshutdown)
- [Actor](#actor-1): [`Actor.reference`](#actorreference), [`Actor.stop`](#actorstop)
- [ActorRef](#actorref): [`ActorRef.send`](#actorrefsend), [`ActorRef.trySend`](#actorreftrysend)

### Functions

#### pool

```tondo
pub fn pool(workers: Int, capacity: Int): Pool ! ExecutorError
```

A pool that runs up to `workers` jobs at a time and keeps up to
`capacity` more waiting for a worker; fewer than one worker or a negative
capacity is an error.

#### actor

```tondo
pub fn actor[S, M, E](
    state: S,
    capacity: Int,
    handle: fn(var S, M): !E,
): Actor[M, E] ! ExecutorError
```

An actor that starts with `state` and changes it with `handle`, one
message at a time, keeping at most `capacity` messages waiting. An
error of `handle` stops it, and `stop` gives that error.

### ExecutorError

```tondo
pub enum ExecutorError {
    /// A pool runs at least one job at a time: the number given.
    InvalidWorkers(Int)
    /// A capacity cannot be negative: the number given.
    InvalidCapacity(Int)
}
```

Why a pool or an actor cannot be made.

```tondo
import std.console
import std.executor

match executor.pool(0, 8) {
    ok(pool) => pool.shutdown()
    err(executor.ExecutorError.InvalidWorkers(given)) => console.println("{given} workers is too few")
    err(error) => console.println("{error}")
}
```

Implements `Display`.

### SubmitError

```tondo
pub enum SubmitError {
    /// Every worker is busy and the queue is full; only `trySubmit` says it.
    Saturated
    /// The pool was shut down.
    Closed
}
```

Why a pool did not take a job.

```tondo
import std.console
import std.executor

let pool = executor.pool(1, 0)?
let first = pool.trySubmit(() { 1 })?
match pool.trySubmit(() { 2 }) {
    ok(job) => console.println("{job.wait()}")
    err(executor.SubmitError.Saturated) => console.println("busy: try later")
    err(executor.SubmitError.Closed) => console.println("shut down")
}
console.println("{first.wait()}")
pool.shutdown()
```

Implements `Display`.

### SendError

```tondo
pub enum SendError {
    /// The mailbox is full; only `trySend` says it.
    Saturated
    /// The actor stopped: `stop`, the end of its owner's block, or an
    /// error of its handler.
    Stopped
}
```

Why an actor did not take a message.

```tondo
import std.console
import std.executor

let printer = executor.actor(0, 4, (count: var Int, line: String) {
    count += 1
    console.println("{count}: {line}")
})?
let address = printer.reference()
address.send("first")?
printer.stop()?
match address.send("late") {
    ok(_) => console.println("sent")
    err(executor.SendError.Stopped) => console.println("the actor stopped")
    err(error) => console.println("{error}")
}
```

Implements `Display`.

### Pool

```tondo
pub type Pool = {
    // private fields
}
```

Runs at most `workers` jobs at a time and keeps at most `capacity` more
waiting for a worker; a handle its copies share. Jobs are tasks whose
`Join` the caller owns: the block that holds one waits for it, and
`job.cancel()` or a `Group` cancel it, also while it waits for a worker.

```tondo
import std.console
import std.executor

// Blocking work, as a C library's, on two threads of its own.
let pool = executor.pool(2, 16)?
let checksum = pool.submit(() { 6 * 7 })?
console.println("{checksum.wait()}")
pool.shutdown()
```

#### Pool.submit

```tondo
pub fn Pool.submit[T](self, job: fn(): T): Join[T] ! SubmitError
```

Starts `job` as a task that runs once a worker is free, waiting first
while the pool is full: its `Join`.

#### Pool.trySubmit

```tondo
pub fn Pool.trySubmit[T](self, job: fn(): T): Join[T] ! SubmitError
```

Like `submit`, without waiting: `SubmitError.Saturated` while the pool
is full.

#### Pool.shutdown

```tondo
pub fn Pool.shutdown(self)
```

Takes no more jobs, and waits until those it took have finished; a
`submit` that waited for room fails with `SubmitError.Closed`.

### Actor

```tondo
pub type Actor[M, E] = {
    // private fields
}
```

State of its own that one task changes, a message at a time and in the
order they arrived. `stop` ends it; so does the end of the block that
owns it, as that block waits for a `Join` nobody consumed: the actor
handles what its mailbox holds, and the block waits for that.

```tondo
import std.console
import std.executor

let totals = executor.actor(0, 16, (total: var Int, amount: Int) {
    total += amount
    console.println("total {total}")
})?
totals.reference().send(5)?
totals.stop()?
```

#### Actor.reference

```tondo
pub fn Actor[M, E].reference(self): ActorRef[M]
```

An address to send the actor messages, which does not keep it alive.

#### Actor.stop

```tondo
pub fn Actor[M, E].stop(self): !E
```

Handles the messages already in the mailbox, then ends: the error of the
handler, if one stopped it. Stopping it again gives the same.

### ActorRef

```tondo
pub type ActorRef[M] = {
    // private fields
}
```

Where an actor's messages go, a handle its copies share; it does not
keep the actor alive.

```tondo
import std.executor

fn report(to: executor.ActorRef[String]): !executor.SendError {
    to.send("done")?
}

let log = executor.actor(0, 8, (_: var Int, _: String) {})?
report(log.reference())?
log.stop()?
```

#### ActorRef.send

```tondo
pub fn ActorRef[M].send(self, message: M): !SendError
```

Puts `message` in the mailbox, waiting while it is full.

#### ActorRef.trySend

```tondo
pub fn ActorRef[M].trySend(self, message: M): !SendError
```

Like `send`, without waiting: `SendError.Saturated` while the mailbox
is full.
