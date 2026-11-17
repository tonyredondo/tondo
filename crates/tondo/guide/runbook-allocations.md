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

/// Counts each thing bought, by its name and its price, into `seen`.
fn tally(seen: var Map[String, Int], lines: Array[Line]) {
    for line in lines {
        seen["{line.name} at {line.cents}", default: 0] += 1
    }
}

fn main() {}
```

```file src/main_test.to
import std.testing

fn bought(): Array[Line] {
    (0..100).map((n) { Line { name: "thing {n % 10}", cents: n % 10 * 5 } }).collect()
}

test everyThingIsCounted {
    let lines = bought()
    var seen: Map[String, Int] = [:]
    tally(var seen, lines)
    assert(seen["thing 3 at 15"] == 10)
    testing.bench("tally", () { tally(var seen, lines) })?
}
```

A benchmark's row gives the allocations of one call and their bytes beside
its time. `save_baseline` keeps the row, to compare with after the change.

```mcp
test {"bench": true, "bench_time": 1, "save_baseline": "before"}
=> 100 allocs/op
```

A hundred lines, and a hundred allocations, though the map already has every
key. `allocs/op` is the heap's. What a release build can let go of all at
once it puts in a region by itself, and a row counts that apart, as `+101 in
regions` after its `allocs/op`: those are cheap, and nothing in the program
is to change for them.
`profile` with `bench` and the kind `memory` measures what the benchmark runs
and gives the lines that allocate, the program's own first.

```mcp
profile {"bench": true, "kind": "memory"}
=> src/main.to:6
```

Line 6 makes them all. `optimizations` says what the compiler made of it:
`kind` `heap` keeps what is made on the heap, each with what keeps it there,
`function` the lines of one function, and `tests` reads the code as the tests
and benchmarks build it, which is how a function that only they call is seen.

```mcp
optimizations {"tests": true, "function": "tally", "kind": "heap"}
=> `"{line.name} at {line.cents}"` is made on the heap: a collection takes it
```

Each line's text is made to be a key of a map that the caller keeps, so it
has to be on the heap, and it is made again for a key the map already has.
The text adds nothing to its two parts: a map takes them as its key, in a
tuple, and nothing is made.

```edit src/main.to
-fn tally(seen: var Map[String, Int], lines: Array[Line]) {
+fn tally(seen: var Map[(String, Int), Int], lines: Array[Line]) {
```

```edit src/main.to
-        seen["{line.name} at {line.cents}", default: 0] += 1
+        seen[(line.name, line.cents), default: 0] += 1
```

```edit src/main_test.to
-    var seen: Map[String, Int] = [:]
+    var seen: Map[(String, Int), Int] = [:]
```

```edit src/main_test.to
-    assert(seen["thing 3 at 15"] == 10)
+    assert(seen[("thing 3", 15)] == 10)
```

`baseline` compares with the row kept, and `optimizations` has nothing left
to say of the function's heap.

```mcp
test {"bench": true, "bench_time": 1, "baseline": "before"}
=> 0 allocs/op
optimizations {"tests": true, "function": "tally", "kind": "heap"}
=> nothing to tell
```

A benchmark is looked at now and then, and a test runs at every change. So
the number goes into a test: `testing.allocations` gives how many times a
call allocates, on the heap and in regions: the benchmark's `allocs/op` and
what its row says `in regions`, added. A plain `test` fails the day the
tally makes a text for each line again, wherever those texts would go.

```edit src/main_test.to
-test everyThingIsCounted {
+test theTallyAllocatesNothing {
+    let lines = bought()
+    var seen: Map[(String, Int), Int] = [:]
+    tally(var seen, lines)
+    assert(testing.allocations(() { tally(var seen, lines) }) == 0)
+}
+
+test everyThingIsCounted {
```

```mcp
test {}
=> theTallyAllocatesNothing ... ok
```

What the view says of a value on the heap, and what it asks for; after the
reason, the code that keeps the value and where it is, as `` `all.push(line)`
at 6:5 ``, when it is the program's:

- **it is returned**, **a collection takes it**: the value outlives the code
  that makes it, so the allocation is the program's. Ask whether the value
  has to be made: here the text was only a key, and its parts were key
  enough.
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
