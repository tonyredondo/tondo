# std.tasks

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.tasks` shows the same in a terminal.

- [Functions](#functions): [`limit`](#limit)
- [Limit](#limit-1)

### Functions

#### limit

```tondo
pub fn limit(places: Int): Limit
```

A limit of `places` tasks alive at once, for `within`: each task that
the block's own task starts, not detached, takes a place before it
starts, and gives it back as it ends. Panics when `places` is below 1.

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
