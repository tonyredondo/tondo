# Runbook: something is slow

Measure before changing anything: the slow part is rarely the one that looks
slow. `profile` says where the time goes, line by line.

```tondo file=slow.to
import std.console

fn isPrime(number: Int): Bool {
    for divisor in 2..number {
        if number % divisor == 0 {
            return false
        }
    }
    number > 1
}

fn main() {
    var found = 0
    for number in 0..60000 {
        if isPrime(number) {
            found += 1
        }
    }
    console.println("{found}")
}
```

```mcp
profile {"path": "slow.to"}
=> slow.to:5
=> isPrime
```

`kind` asks `memory`, what it allocates, or `tasks`, where they wait. `top` gives
more lines, and `profile_show` reads a profile written before; with `asm`, the
instructions of the function that took the most, each with its share. A
sample often falls on the instruction after the slow one, which waits for it.

`optimizations` says what a release build made of that code: the calls it
inlined and those it kept, with why; the texts and closures it made without the
heap, and what keeps the others on it, naming the code; the checks it dropped,
and those a loop keeps, with the bound it lacked, as "nothing bounds `count`
from above": a value compared with a limit before the operation gives one.
What it decided in the standard library's code a loop calls is said at the
program's call, as ``in `prelude.CharIterator.next` `` with that code.
`function` keeps one function, with the code written in it wherever it
went, `kind` one kind of line, as `heap` for what allocates, and `against`
with a commit only what a change made different.

```mcp
optimizations {"path": "slow.to"}
=> `isPrime(number)` is inlined
```

`asm` shows what the processor runs for a function, instruction by
instruction, under the lines of the program they are of: here `main`, with
`isPrime` inlined in it and the division of its hot line.

```mcp
asm {"path": "slow.to", "function": "main"}
=> slow.to:5  if number % divisor == 0 {
```

For a number to compare, a benchmark is a test that calls `testing.bench`.

```sh
tondo init
```

```tondo file=src/main.to
fn isPrime(number: Int): Bool {
    for divisor in 2..number {
        if number % divisor == 0 {
            return false
        }
    }
    number > 1
}

fn main() {}
```

```file src/main_test.to
import std.testing

test primes {
    testing.bench("isPrime", () { isPrime(10007) })?
}
```

`save_baseline` keeps the numbers, and `baseline` compares with them after a
change; `against` compares with a commit, and `max_regression` fails if it got
worse, which is what CI wants.

```mcp
test {"bench": true, "bench_time": 1, "save_baseline": "before"}
=> isPrime
```

A divisor above the square root has a partner below it, so that is far enough.

```edit src/main.to
-    for divisor in 2..number {
-        if number % divisor == 0 {
-            return false
-        }
-    }
+    var divisor = 2
+    for divisor * divisor <= number {
+        if number % divisor == 0 {
+            return false
+        }
+        divisor += 1
+    }
```

```mcp
test {"bench": true, "bench_time": 1, "baseline": "before"}
=> isPrime
```

A row's time moves by a third when only where the code lies moves, as after
an unrelated change. On Linux a row also has the instructions and cycles of
a run, counted in a round of their own so that the time is measured with
nothing counting: `regression_by` `instructions` judges a change by them, and
`layouts` measures in several orders of the code and gives the range. A
machine busier than its processors moves the time too, and the report ends
with a note that says so: then compare instructions, not times. A program that
holds a `recover`, as a project's tests may, checks for a panic after what may
raise one, which `optimizations` with `tests` says in a loop: a benchmark's
row then costs more than the same loop in a program without one. After
taking a new `tondo`, `compiler` with the old one, in `test` with `bench`, in
`optimizations` and in `asm`, says what the new one makes differently.

When the row says the cost is in allocations: `runbook-allocations`. For a
program that is slow as it runs, without stopping it: `runbook-running`. To
find the commit that made a benchmark slower: `runbook-bisect`.
