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
heap, and what keeps the others on it; the checks it dropped, and those a loop
keeps. Its `kind` `heap` gives only what allocates.

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
