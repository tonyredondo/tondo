# Runbook: from a dump to a test

A failure was seen once and must not come back. `dump_test` writes a test that
calls the function that failed with the values the dump says it had, so the
failure can be repeated before it is fixed. It needs a project, where a test
lives, and runs where the program's sources are: the server's directory is the
project. `new_project` with `init` makes one of a directory.

```sh
tondo init
```

```tondo file=src/main.to
import std.console

type Order = { id: Int, lines: Array[Int] }

fn total(order: Order): Int {
    var sum = 0
    for at in 0..order.lines.length() + 1 {
        sum += order.lines[at]
    }
    sum
}

fn main() {
    let order = Order { id: 7, lines: [10, 20] }
    console.println("{total(order)}")
}
```

```file src/main_test.to
test sums {
    assert(total(Order { id: 1, lines: [] }) == 0)
}
```

```sh
tondo build --output shop
mkdir dumps
TONDO_DUMP=dumps ./shop
```

`dump_test` writes the test next to the function and runs it. `reproduces`
says whether it fails as the dump did; `false` means the values alone do not
bring the failure back, and `frame` tries a function further up the stack.

```mcp
dump_test {"file": "<dump>"}
=> "reproduces":true
=> failedInTotal
=> `test` shows it failing
```

`test` now shows the failure as a test, with the same stack.

```mcp
test {"filter": "failedInTotal"}
=> failedInTotal ... FAILED
=> panic[P0001]: bounds
```

Fix the code, not the test: the test is what proves the fix.

```edit src/main.to
-    for at in 0..order.lines.length() + 1 {
+    for at in 0..order.lines.length() {
```

```mcp
test {}
=> failedInTotal ... ok
```

Give the test a name that says what it guards, and keep it.
