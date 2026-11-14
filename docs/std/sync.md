# std.sync

What tasks share. Tondo's values are copied, so a task cannot change
another's data by accident, and a plain `var` cannot be given to two tasks
to change: that does not compile. State that really is shared is put in
one of the types of this module, each of which is a handle that its copies
share, and each of which makes its rule impossible to forget, since the
value is inside it.

- [A value behind a lock](#a-value-behind-a-lock)
- [Waiting for a condition](#waiting-for-a-condition)
- [Which to use](#which-to-use)

## A value behind a lock

```tondo
import std.console
import std.sync

type Account = { balance: Int, moves: Int }

fn deposit(account: sync.Mutex[Account], amount: Int) {
    // The function has the value to itself while it runs.
    account.update((now) {
        now.balance += amount
        now.moves += 1
    })
}

fn main() {
    let account = sync.Mutex(Account { balance: 0, moves: 0 })
    let visits = sync.Atomic(0)
    var group = Group[Unit]()
    for amount in 1..=100 {
        group.add(spawn deposit(account, amount))
        visits.add(1)
    }
    group.wait()
    let end = account.get()
    console.println("{end.balance} in {end.moves} moves, {visits.load()} visits")

    // Reading and then writing is two steps, and another task can come between them.
    let withdrawn = account.update((now) {
        if now.balance >= 5000 {
            now.balance -= 5000
            true
        } else {
            false
        }
    })
    console.println("{withdrawn} {account.get().balance}")
}
```

```text output
5050 in 100 moves, 100 visits
true 50
```

- [`Mutex.update`](#mutexupdate) is the way in. There is no `lock` without
  its `unlock`, no way to read the value without the lock, and a panic or a
  cancellation inside lets the lock go.
- `update` gives back what its function gives, so a check and the change
  it allows are one step, as the withdrawal above.
- [`Atomic`](#atomic) is for one number or flag that many tasks touch:
  `add`, `load`, `store`, and `compareExchange` for the rest.
- `tondo test` runs a test that starts tasks in the orders that could
  change its result, so the lost update of a `get` followed by a `set`
  fails in a plain run ([Testing](../toolchain/testing.md#tests-that-start-tasks)).

## Waiting for a condition

[`Mutex.updateWhen`](#mutexupdatewhen) waits until a condition holds for
the value and then changes it, as one step. It is what a condition
variable is for, without the loop around it:

```tondo
import std.console
import std.sync

/// Takes the next job, waiting while there is none.
fn take(jobs: sync.Mutex[Array[String]]): String {
    jobs.updateWhen((waiting) { not waiting.isEmpty() }, (waiting) { waiting.removeAt(0) ?? "" })
}

fn main() {
    let empty: Array[String] = []
    let jobs = sync.Mutex(empty)
    let worker = spawn take(jobs)               // waits: there is nothing yet
    jobs.update((waiting) { waiting.push("resize the images") })
    console.println(worker.wait())
}
```

```text output
resize the images
```

## Which to use

| For | Use |
| --- | --- |
| A value that several tasks change | [`Mutex`](#mutex) |
| One that is mostly read | [`RwLock`](#rwlock) |
| A counter or a flag | [`Atomic`](#atomic) |
| Something made once, by whoever asks first | [`Once`](#once) |
| At most so many at a time | [`Semaphore`](#semaphore) |
| At most so many a second | [`RateLimiter`](#ratelimiter) |
| Tasks that start together | [`Barrier`](#barrier) |
| A map, a set or a queue that many tasks use | [`Map`](#map), [`Set`](#set), [`Queue`](#queue), [`Stack`](#stack), [`Array`](#array), which need no lock around them |
| Doing something again until it works | [`retry`](#retry) |

Passing values between tasks is a channel, in the language, and often the
better design: [Tasks](../language/13-tasks.md) has both, and when to
choose which. The [`warehouse`](../../examples/warehouse) example keeps
its state in a `Mutex`, [`chat`](../../examples/chat) gives its state to an
actor instead, and [`linkcheck`](../../examples/linkcheck) shares a rate
limiter.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.sync` shows the same in a terminal.

- [Functions](#functions): [`retry`](#retry)
- [CompareExchange](#compareexchange)
- [Array](#array): [`Array.length`](#arraylength), [`Array.get`](#arrayget), [`Array.set`](#arrayset), [`Array.compareExchange`](#arraycompareexchange), [`Array.snapshot`](#arraysnapshot)
- [Map](#map): [`Map.length`](#maplength), [`Map.get`](#mapget), [`Map.contains`](#mapcontains), [`Map.insert`](#mapinsert), [`Map.remove`](#mapremove), [`Map.compareExchange`](#mapcompareexchange), [`Map.snapshot`](#mapsnapshot)
- [Set](#set): [`Set.length`](#setlength), [`Set.contains`](#setcontains), [`Set.insert`](#setinsert), [`Set.remove`](#setremove), [`Set.snapshot`](#setsnapshot)
- [Stack](#stack): [`Stack.push`](#stackpush), [`Stack.pop`](#stackpop), [`Stack.get`](#stackget), [`Stack.snapshot`](#stacksnapshot)
- [Queue](#queue): [`Queue.push`](#queuepush), [`Queue.popFront`](#queuepopfront), [`Queue.get`](#queueget), [`Queue.snapshot`](#queuesnapshot)
- [RateLimiter](#ratelimiter): [`RateLimiter`](#ratelimiter-1), [`RateLimiter.allow`](#ratelimiterallow), [`RateLimiter.wait`](#ratelimiterwait)
- [Mutex](#mutex): [`Mutex`](#mutex-1), [`Mutex.update`](#mutexupdate), [`Mutex.tryUpdate`](#mutextryupdate), [`Mutex.updateWhen`](#mutexupdatewhen), [`Mutex.get`](#mutexget), [`Mutex.set`](#mutexset)
- [RwLock](#rwlock): [`RwLock`](#rwlock-1), [`RwLock.read`](#rwlockread), [`RwLock.update`](#rwlockupdate), [`RwLock.get`](#rwlockget), [`RwLock.set`](#rwlockset)
- [Semaphore](#semaphore): [`Semaphore`](#semaphore-1), [`Semaphore.acquire`](#semaphoreacquire), [`Semaphore.tryAcquire`](#semaphoretryacquire)
- [Permit](#permit): [`Permit.release`](#permitrelease)
- [Once](#once): [`Once`](#once-1), [`Once.getOrInit`](#oncegetorinit), [`Once.get`](#onceget), [`Once.isReady`](#onceisready)
- [Barrier](#barrier): [`Barrier`](#barrier-1), [`Barrier.wait`](#barrierwait)
- [Atomic](#atomic): [`Atomic`](#atomic-1), [`Atomic.load`](#atomicload), [`Atomic.store`](#atomicstore), [`Atomic.swap`](#atomicswap), [`Atomic.compareExchange`](#atomiccompareexchange), [`Atomic.add`](#atomicadd)

### Functions

#### retry

```tondo
pub fn retry[T: ~Copy, E: ~Copy](
    work: fn(): T ! E,
    attempts: Int = 3,
    delay: time.Duration = time.milliseconds(100),
    maxDelay: time.Duration = time.seconds(10),
    when: (fn(ref E): Bool)? = none,
): T ! E
```

The work again until it succeeds, `attempts` times at most. Between
two, it waits a random part of a delay that doubles from `delay` up to
`maxDelay`, so that many clients do not try again together. Only the
errors `when` accepts are tried again; the last error is the result. A
task cancelled while it waits stops.

### CompareExchange

```tondo
pub enum CompareExchange[T] {
    Exchanged(T)
    Mismatch(T)
}
```

What `compareExchange` did: `Exchanged(previous)` when the value was the
one expected and was replaced, `Mismatch(observed)` when it was not.

```tondo
import std.console
import std.sync

let highest = sync.Atomic(10)
let offered = 25
for {
    let current = highest.load()
    if offered <= current {
        break
    }
    match highest.compareExchange(current, offered) {
        sync.CompareExchange.Exchanged(previous) => {
            console.println("raised from {previous}")
            break
        }
        sync.CompareExchange.Mismatch(_) => {}
    }
}
console.println("highest {highest.load()}")
```

### Array

```tondo
pub type Array[T]
```

Fixed length.

```tondo
import std.console
import std.sync

fn count(slots: sync.Array[Int], index: Int) {
    _ = slots.set(index, index * index)
}

let slots = sync.Array([0, 0, 0, 0])
var group = Group[Unit]()
for index in 0..4 {
    group.add(spawn count(slots, index))
}
group.wait()
console.println("{slots.snapshot()} of {slots.length()}")
```

#### Array.length

```tondo
pub fn Array.length(self): Int
```

The number of elements, which never changes.

#### Array.get

```tondo
pub fn Array.get(self, index: Int): T?
```

The element at `index`, counting from the end when it is negative, or
`none` outside the array.

#### Array.set

```tondo
pub fn Array.set(self, index: Int, value: T): T ! collections.CollectionError
```

Replaces the element at `index` and gives the one it had; an index
outside the array is `CollectionError.InvalidIndex`.

#### Array.compareExchange

```tondo
pub fn Array.compareExchange(self, index: Int, expected: T, desired: T): CompareExchange[T] ! collections.CollectionError
```

Replaces the element at `index` with `desired` if it is `expected`, as
`Atomic.compareExchange` does.

#### Array.snapshot

```tondo
pub fn Array.snapshot(self): Array[T]
```

A copy of the elements at one instant.

### Map

```tondo
pub type Map[K: Key, V]
```

A map that tasks share, read without locks: `sync.Map["a": 1]`,
`sync.Map[K, V]()` or `sync.Map(entries)`.

```tondo
import std.console
import std.sync

fn visit(seen: sync.Map[String, Int], page: String) {
    for {
        let before = seen.get(page)
        let after = some((before ?? 0) + 1)
        match seen.compareExchange(page, before, after) {
            sync.CompareExchange.Exchanged(_) => break
            sync.CompareExchange.Mismatch(_) => {}
        }
    }
}

let seen = sync.Map[String, Int]()
var group = Group[Unit]()
for page in ["/", "/docs", "/"] {
    group.add(spawn visit(seen, page))
}
group.wait()
console.println("{seen.get("/")} {seen.contains("/docs")} {seen.length()}")
```

Implements `reflect.Reflect`.

#### Map.length

```tondo
pub fn Map.length(self): Int
```

The number of entries: exact without concurrent writes, and with them,
one that may count some of them.

#### Map.get

```tondo
pub fn Map.get(self, key: K): V?
```

The value of `key`, or `none` when the map does not have it.

#### Map.contains

```tondo
pub fn Map.contains(self, key: K): Bool
```

Whether the map has `key`.

#### Map.insert

```tondo
pub fn Map.insert(self, key: K, value: V): V?
```

Puts `value` under `key` and gives the value it replaced, if any.

#### Map.remove

```tondo
pub fn Map.remove(self, key: K): V?
```

Removes `key` and gives its value, or `none` when the map did not have
it. The map keeps nothing of it: a channel's end it gives closes once
the program drops it, as a local's does.

#### Map.compareExchange

```tondo
pub fn Map.compareExchange(self, key: K, expected: V?, desired: V?): CompareExchange[V?]
```

`expected = none` compares with absence; `desired = none` removes.

#### Map.snapshot

```tondo
pub fn Map.snapshot(self): Map[K, V]
```

A copy of the entries at one instant, in no particular order.

### Set

```tondo
pub type Set[K: Key]
```

A set that tasks share, read without locks: `sync.Set["a"]`,
`sync.Set[K]()` or `sync.Set(keys)`.

```tondo
import std.console
import std.sync

fn claim(taken: sync.Set[String], name: String): Bool {
    taken.insert(name)
}

let taken = sync.Set[String]()
var group = Group[Bool]()
for name in ["ada", "bo", "ada"] {
    group.add(spawn claim(taken, name))
}
let claimed = group.wait()
console.println("{claimed.filter((won) { won }).length()} claimed, {taken.snapshot()}")
```

#### Set.length

```tondo
pub fn Set.length(self): Int
```

The number of values: exact without concurrent writes, and with them,
one that may count some of them.

#### Set.contains

```tondo
pub fn Set.contains(self, key: K): Bool
```

Whether the set has `key`.

#### Set.insert

```tondo
pub fn Set.insert(self, key: K): Bool
```

Adds `key`: `true` when it was new.

#### Set.remove

```tondo
pub fn Set.remove(self, key: K): Bool
```

Removes `key`: `true` when the set had it.

#### Set.snapshot

```tondo
pub fn Set.snapshot(self): Set[K]
```

A copy of the values at one instant, in no particular order.

### Stack

```tondo
pub type Stack[T]
```

LIFO, the top at the end.

```tondo
import std.console
import std.sync

let undo = sync.Stack[String]()
undo.push("type a")
undo.push("type b")
console.println("{undo.pop()} then {undo.get(-1)}; left {undo.snapshot()}")
```

#### Stack.push

```tondo
pub fn Stack.push(self, value: T)
```

Adds the value on top.

#### Stack.pop

```tondo
pub fn Stack.pop(self): T?
```

Removes the value on top and gives it, or `none` at once when the stack
is empty. The stack keeps nothing of it, as `sync.Map.remove`.

#### Stack.get

```tondo
pub fn Stack.get(self, index: Int): T?
```

The value at `index` from the bottom, counting from the top when it is
negative, or `none` outside the stack.

#### Stack.snapshot

```tondo
pub fn Stack.snapshot(self): Array[T]
```

A copy of the values at one instant, from the bottom to the top.

### Queue

```tondo
pub type Queue[T]
```

FIFO with many producers and consumers.

```tondo
import std.console
import std.sync

fn work(jobs: sync.Queue[Int], done: sync.Atomic[Int]) {
    for {
        match jobs.popFront() {
            some(job) => _ = done.add(job)
            none => break
        }
    }
}

let jobs = sync.Queue[Int]()
for job in 1..=10 {
    jobs.push(job)
}
let done = sync.Atomic(0)
var group = Group[Unit]()
for _ in 0..3 {
    group.add(spawn work(jobs, done))
}
group.wait()
console.println("sum {done.load()}")
```

#### Queue.push

```tondo
pub fn Queue.push(self, value: T)
```

Adds the value at the back.

#### Queue.popFront

```tondo
pub fn Queue.popFront(self): T?
```

Removes the value at the front and gives it, or `none` at once when the
queue is empty. The queue keeps nothing of it, as `sync.Map.remove`.

#### Queue.get

```tondo
pub fn Queue.get(self, index: Int): T?
```

The value at `index` from the front, counting from the back when it is
negative, or `none` outside the queue.

#### Queue.snapshot

```tondo
pub fn Queue.snapshot(self): Deque[T]
```

A copy of the values at one instant, from the front to the back.

### RateLimiter

```tondo
pub type RateLimiter = {
    // private fields
}
```

Lets work happen at a steady rate, `rate` times each `per`, and up to
`burst` at once after a quiet while, as a token bucket does. Its copies
share it, so the tasks of a server share one.

```tondo
import std.console
import std.sync
import std.time

let limiter = sync.RateLimiter(5, per: time.seconds(1), burst: 2)
var allowed = 0
for _ in 0..10 {
    if limiter.allow() {
        allowed += 1
    }
}
console.println("{allowed} of 10 at once")
limiter.wait()
console.println("one more after waiting")
```

#### RateLimiter

```tondo
pub fn RateLimiter(
    rate: Int,
    per: time.Duration = time.seconds(1),
    burst: Int? = none,
): RateLimiter
```

A limiter of `rate` each `per`, one second by default, with bursts of
`burst`, or of `rate` without it.

#### RateLimiter.allow

```tondo
pub fn RateLimiter.allow(self): Bool
```

Whether one more may happen now, taking its place when so.

#### RateLimiter.wait

```tondo
pub fn RateLimiter.wait(self)
```

Waits until one more may happen, and takes its place; a task cancelled
while it waits stops waiting.

### Mutex

Implements `reflect.Reflect`.

#### Mutex

```tondo
pub fn Mutex(value: T): Mutex[T]
```

A value behind a lock; closures see it while they hold the lock.

```tondo
import std.console
import std.sync

let totals = sync.Mutex(["apples": 0])
totals.update((map) { map["apples"] = (map["apples"] ?? 0) + 3 })
console.println("{totals.get()}")
```

#### Mutex.update

```tondo
pub fn Mutex.update(self, change: fn(var T): R): R
```

Runs `change` with the value locked for this task alone, and gives its
result; the lock is let go when it returns, also on cancellation or a
panic, which leaves it free for the next.

#### Mutex.tryUpdate

```tondo
pub fn Mutex.tryUpdate(self, change: fn(var T): R): R?
```

`none` without waiting when another holds the lock.

#### Mutex.updateWhen

```tondo
pub fn Mutex.updateWhen(self, ready: fn(T): Bool, change: fn(var T): R): R
```

Waits until `ready` holds, then changes the value under the same lock.

#### Mutex.get

```tondo
pub fn Mutex.get(self): T
```

A copy of the value, read under the lock.

#### Mutex.set

```tondo
pub fn Mutex.set(self, value: T)
```

Replaces the value under the lock.

### RwLock

Implements `reflect.Reflect`.

#### RwLock

```tondo
pub fn RwLock(value: T): RwLock[T]
```

Many readers or one writer.

#### RwLock.read

```tondo
pub fn RwLock.read(self, observe: fn(T): R): R
```

Runs `observe` with the value locked for reading, which other readers
share, and gives its result.

#### RwLock.update

```tondo
pub fn RwLock.update(self, change: fn(var T): R): R
```

Runs `change` with the value locked for this task alone, and gives its
result.

#### RwLock.get

```tondo
pub fn RwLock.get(self): T
```

A copy of the value, read under a read lock.

#### RwLock.set

```tondo
pub fn RwLock.set(self, value: T)
```

Replaces the value under the write lock.

### Semaphore

Implements `reflect.Reflect`.

#### Semaphore

```tondo
pub fn Semaphore(permits: Int): Semaphore
```

A semaphore with `permits` permits; a number below one panics.

#### Semaphore.acquire

```tondo
pub fn Semaphore.acquire(self): Permit
```

Waits for a free permit and takes it; it comes back when the `Permit` is
released or dropped.

#### Semaphore.tryAcquire

```tondo
pub fn Semaphore.tryAcquire(self): Permit?
```

A free permit, or `none` at once when there is none.

### Permit

#### Permit.release

```tondo
pub fn Permit.release(self)
```

A permit goes back when released or abandoned.

### Once

#### Once

```tondo
pub fn Once[T](): Once[T]
```

A value initialised once; failures leave it empty for the next try.

#### Once.getOrInit

```tondo
pub fn Once.getOrInit(self, init: fn(): T): T
```

The value, made by `init` the first time; other tasks wait meanwhile,
and when `init` fails or is cancelled, the next call tries again.

#### Once.get

```tondo
pub fn Once.get(self): T?
```

The value, or `none` before it is made.

#### Once.isReady

```tondo
pub fn Once.isReady(self): Bool
```

Whether the value is made.

### Barrier

#### Barrier

```tondo
pub fn Barrier(parties: Int): Barrier
```

Waits for `parties` tasks; `true` in exactly one of them per round.

#### Barrier.wait

```tondo
pub fn Barrier.wait(self): Bool
```

Waits until the barrier's number of tasks arrive: `true` in exactly one
of them each round; the barrier is then used again.

### Atomic

Implements `reflect.Reflect`.

#### Atomic

```tondo
pub fn Atomic(value: T): Atomic[T]
```

Sequentially consistent; lock-free for integers, `Bool`, `Char`, `Byte`.

#### Atomic.load

```tondo
pub fn Atomic.load(self): T
```

The value.

#### Atomic.store

```tondo
pub fn Atomic.store(self, value: T)
```

Replaces the value.

#### Atomic.swap

```tondo
pub fn Atomic.swap(self, value: T): T
```

Replaces the value and gives the one it had.

#### Atomic.compareExchange

```tondo
pub fn Atomic.compareExchange(self, expected: T, desired: T): CompareExchange[T]
```

Replaces the value with `desired` if it is `expected`: `Exchanged` with
the value it had, or `Mismatch` with the one it has, which never fails
for no reason.

#### Atomic.add

```tondo
pub fn Atomic.add(self, delta: T): T
```

The new value.
