# std.tasks

What goes with the language's tasks: `checkpoint` and `cancelled`, for a
computation that does not wait and has to be stopped
([Tasks](../language/13-tasks.md#a-task-that-only-computes)), and `limit`,
the bound on how many of the tasks a `within` block starts are alive at
once, which reads beside a deadline as its sibling does:

```tondo
import std.tasks
import std.time

fn fetch(id: Int): Int {
    time.sleep(time.milliseconds(5))
    id
}

fn main() {
    let ids = [1, 2, 3, 4, 5, 6]
    let fetched =
        within time.seconds(5), tasks.limit(2) { ids.map((id) { spawn fetch(id) }).wait() }
    _ = fetched
}
```

[Tasks](../language/13-tasks.md#at-most-so-many-at-once) says what a limit
counts, whom it binds, and how one limit is shared.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.tasks` shows the same in a terminal.

- [Functions](#functions): [`limit`](#limit), [`checkpoint`](#checkpoint), [`cancelled`](#cancelled)
- [Limit](#limit-1)

### Functions

#### limit

```tondo
pub fn limit(places: Int): Limit
```

A limit of `places` tasks alive at once, for `within`: each task that
the block's own task starts, not detached, takes a place before it
starts, and gives it back as it ends. Panics when `places` is below 1.

#### checkpoint

```tondo
pub fn checkpoint()
```

A point where this task stops if it was cancelled, or a deadline of the
`within` it runs in passed, as it would at a wait, running its `defer`s;
otherwise it does nothing. For the loop of a computation that does not
wait, which a cancel otherwise lets run to its end (language 11.15).

```tondo
import std.console
import std.tasks
import std.time

let sum = within time.milliseconds(50) {
    var total = 0
    for n in 0..1_000_000_000 {
        if n % 1024 == 0 {
            tasks.checkpoint()
        }
        total += n % 7
    }
    total
}
console.println(if sum.isErr() { "stopped at its deadline" } else { "done" })
```

#### cancelled

```tondo
pub fn cancelled(): Bool
```

Whether this task was cancelled, or a deadline of the `within` it runs
in passed: for a `defer` that tells a cancel from an ordinary end, and
for code that would rather end by itself with what it has. A panic that
a `recover` passes on is no cancel.

```tondo
import std.console
import std.tasks

console.println("{tasks.cancelled()}")
```

### Limit

```tondo
pub type Limit = {
    // private fields
}
```

A limit of how many tasks the `within` blocks of it keep alive at once
(language 11.17). It is a value: every `within` of the same limit shares
its places, so one kept by a server bounds all its requests together.

```tondo
import std.console
import std.tasks

fn twice(n: Int): Int {
    n * 2
}

let fetching = tasks.limit(2)
let first = within fetching { [1, 2, 3].map((n) { spawn twice(n) }).wait() }
let second = within fetching { [4, 5].map((n) { spawn twice(n) }).wait() }
console.println("{first} {second}")
```
