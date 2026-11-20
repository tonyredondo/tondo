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

`schedules` takes a number, so many orders by chance, each with its seed, or
`all`, every order that differs, which says if it got through them all. The
result says which tasks wrote what, and where, and names the order that
failed: `debug_start` with `test` and `schedule` runs that same order again
under the debugger, as many times as it takes.

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

The system leaves moments open too, between two steps that another task can
see: the file of a socket is there a moment before its server listens, and a
signal a program sends itself arrives a moment later. In a real run they last
microseconds, and some orders of `schedules` make them last, so a test that
passes here and fails once in a hundred runs of CI can fail under `schedules`,
with the seed that repeats it. A test that reaches outside the program, to
files, the network or a database, runs in one order in a plain `test`, whose
last line says so: `schedules` tries the others, when the test leaves the
outside as it found it.

`lock_order` finds locks taken in opposite orders, which hang only sometimes,
and `flags` with `--repeat <n>` runs a test many times. A failure that only a
machine with few cores shows, as a runner of CI, is looked for with `flags`
and `--workers <n>`, which runs the tests' tasks on that many workers: `1`
for no two at once, `2` for a small runner. A run recorded with
`record` is the same each time it is replayed, with `debug_start` and
`replay`: `runbook-once`, and `runbook-ci` for a test that fails only there.
