# Runbook: code that allocates more than it should

Each allocation takes time when it is made and again when it is let go, so
code that runs often wants few. Three tools answer in turn: a benchmark counts
them, a profile of memory names the line that makes them, and `optimizations`
says which values a release build made on the heap there, and why.

```sh
tondo init
```

```tondo file=src/main.to
type Line = { name: String, cents: Int }

/// A receipt, a line for each thing bought.
fn receipt(lines: Array[Line]): String {
    lines.map((line) { "{line.name}: {line.cents}\n" }).join("")
}

fn main() {}
```

```file src/main_test.to
import std.testing

fn bought(): Array[Line] {
    (0..100).map((n) { Line { name: "thing {n}", cents: n * 5 } }).collect()
}

test everyLineIsThere {
    let lines = bought()
    assert(receipt(lines).startsWith("thing 0: 0\nthing 1: 5\n"))
    testing.bench("receipt", () { receipt(lines) })?
}
```

A benchmark's row gives the allocations of one call and their bytes beside
its time. `save_baseline` keeps the row, to compare with after the change.

```mcp
test {"bench": true, "bench_time": 1, "save_baseline": "before"}
=> 103 allocs/op
```

A hundred lines, and a hundred and three allocations. `profile` with `bench`
and the kind `memory` measures what the benchmark runs and gives the lines that
allocate, the program's own first.

```mcp
profile {"bench": true, "kind": "memory"}
=> src/main.to:5
```

Line 5 makes them all. `optimizations` says what the compiler made of it:
`kind` `heap` keeps what is made on the heap, each with what keeps it there,
`function` the lines of one function, and `tests` reads the code as the tests
and benchmarks build it, which is how a function that only they call is seen.

```mcp
optimizations {"tests": true, "function": "receipt", "kind": "heap"}
=> `"{line.name}: {line.cents}\n"` is made on the heap: a collection takes it
```

Each line's text is made to be put in an array, which `join` then copies into
one more text: the array and its hundred texts live only to be copied. A
`format.Builder` is one text that grows, and takes each part as it comes.

```edit src/main.to
-type Line = { name: String, cents: Int }
+import std.format
+
+type Line = { name: String, cents: Int }
```

```edit src/main.to
-    lines.map((line) { "{line.name}: {line.cents}\n" }).join("")
+    var text = format.Builder()
+    for line in lines {
+        text.add(line.name)
+        text.add(": ")
+        text.addLine(line.cents)
+    }
+    text.text()
```

`baseline` compares with the row kept: the allocations that are left are the
builder's text as it doubles, whatever the number of lines.

```mcp
test {"bench": true, "bench_time": 1, "baseline": "before"}
=> 8 allocs/op
optimizations {"tests": true, "function": "receipt", "kind": "heap"}
=> nothing to tell
```

Give the builder the parts, not a text made of them: `text.add("{line.name}:
{line.cents}")` makes that text first, at every turn, and the view says so.

What the view says of a value on the heap, and what it asks for:

- **it is returned**, **a collection takes it**: the value outlives the code
  that makes it, so the allocation is the program's. Make fewer of them, as
  here, not none.
- **a value holds it**: a record or a closure keeps it, as the adapters of a
  chain keep their closures. A function that only calls a closure leaves it in
  the frame.
- **it is passed to a call that may keep it**, **it is read in a way that may
  keep it**: the compiler could not see that what receives it only reads it.
  Pass the parts, or what it is made from.

For a loop, the kind `grown` gives the arrays that grow as they are pushed to
and `reserved` those given their room before the loop. `check` with
`performance` warns, while writing, of a copy that a loop makes on every turn.
`against` with a commit, in `optimizations` and in `test` with `bench`, gives
only what a change made different, and `max_regression` fails a run whose
benchmarks allocate more than the baseline's: that is the form for CI.

When it is time rather than allocations: `runbook-performance`. When memory
keeps growing: `runbook-memory`.
