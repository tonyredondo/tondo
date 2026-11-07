# Runbook: a test that fails sometimes

A test that fails now and then usually has tasks that race. Running it again
and again proves little: `schedules` tries many orders of its tasks, and says
which one fails. There are no data races in safe code, but a value read and
written in two steps can lose an update.

```sh
tondo init
```

```tondo file=src/main.to
import std.sync

fn add(count: sync.Mutex[Int]) {
    let now = count.get()
    count.set(now + 1)
}

fn main() {}
```

```file src/main_test.to
import std.sync

test counts {
    let count = sync.Mutex(0)
    let first = spawn add(count)
    let second = spawn add(count)
    _ = first.wait()
    _ = second.wait()
    assert(count.get() == 2)
}
```

`check` often finds it first, with the repair to apply.

```mcp
check {}
=> W1016
```

`--schedules all` tries every order that differs, not only some, and says
if it got through them all.

`schedules` takes a number of orders or `all`: each order has a seed, and the result says which
tasks wrote what, and where. `debug_start` with `test` and `schedule` runs that
same order again under the debugger, as many times as it takes.

```mcp
test {"schedules": "50"}
=> counts ... FAILED
=> that write is lost
```

Read and write it in one step.

```edit src/main.to
-    let now = count.get()
-    count.set(now + 1)
+    count.update((value) { value += 1 })
```

```mcp
test {"schedules": "50"}
=> counts ... ok
```

`lock_order` finds locks taken in opposite orders, which hang only sometimes,
and `flags` with `--repeat <n>` runs a test many times. A run recorded with
`record` is the same each time it is replayed, with `debug_start` and `replay`.
