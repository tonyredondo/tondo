# Runbook: a crash

A program ended with a panic or a crash, here or somewhere else. The goal is
the line that failed and the values that made it fail, before changing code.

## With the program at hand

```tondo file=orders.to
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

`run` says all a panic knows: its code, its words and where each call was.
Often that is the whole answer, and nothing else is needed. The program also
left a dump, with what each task held, which `dump_read` reads (below).

```mcp
run {"path": "orders.to"}
=> panic[P0001]: bounds: index 2 is out of bounds
=> orders.to:8:16
=> a dump of the program is in
=> next: `dump_read` reads the dump it left
```

`explain` says what a code means and what usually causes it.

```mcp
explain {"code": "P0001"}
=> An index or bound outside the range allowed
```

## With a dump

A program that panics, faults or deadlocks leaves a dump, with the values each
task held when it ended, and its last line of error output says where: in
`~/.cache/tondo/dumps`, the latest 20, or where `TONDO_DUMP` says, as here, to
keep them together. Of a program that ran elsewhere, the dump is all there is.

```sh
tondo build orders.to --output orders
mkdir dumps
TONDO_DUMP=dumps ./orders
```

`dump_group` comes first with a directory: it sorts the dumps by failure, the
most frequent first, so that you work on what matters and on each one once.

```mcp
dump_group {"dir": "dumps"}
=> orders.to:8 in total
```

`dump_read` with `why` says what stands out; `locals` gives the values of the
frame that failed, which is what `run` cannot show.

```mcp
dump_read {"file": "<dump>", "view": "why"}
=> index 2 is out of bounds
dump_read {"file": "<dump>", "view": "locals"}
=> .lines
```

The index is 2 and `lines` has two values: the loop goes one too far.

## Fix it

```edit orders.to
-    for at in 0..order.lines.length() + 1 {
+    for at in 0..order.lines.length() {
```

```mcp
check {"path": "orders.to"}
=> it compiles
run {"path": "orders.to"}
=> 30
```

A failure worth keeping out for good goes on to `runbook-regression`. A dump to
share is `dump_redact` first: it leaves where the program was, not what it held.
