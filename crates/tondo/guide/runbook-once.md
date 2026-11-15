# Runbook: a failure that happened once

A run went wrong because of what it was given: an input nobody kept, the time
it ran at, the order its tasks took. Running it again gives another run. A
recording keeps what the program read, the time it saw and the order of its
tasks, so that the same run can be asked questions, as many times as it takes.

```tondo file=till.to
import std.console

/// The cents of a sale, as the till writes it: `tea,250`.
fn cents(sale: String): Int {
    match sale.splitOnce(",") {
        some((_, written)) => match Int.parse(written.trim()) {
            ok(value) => value
            err(_) => 0
        }
        none => 0
    }
}

fn main() {
    var total = 0
    for {
        match console.readLine() {
            some(sale) => {
                let amount = cents(sale)
                total += amount
            }
            none => break
        }
    }
    console.println("total {total}")
}
```

`run` with `record` keeps the run in a directory, to copy anywhere. Record what
may fail before it does: `last`, as `5m`, keeps only the end of a long run, and
nothing of one that goes well.

```mcp
run {"path": "till.to", "stdin": "tea,250\ncake,4.00\nwater,100\n", "record": "day.rec"}
=> total 350
=> the run is recorded in day.rec
```

The day's total is 350, and more than that was sold. `debug_start` with
`replay` runs the recording again, the same each time, and never touches the
world: no input to give it, no file it writes. Stop where the total is printed.

```mcp
debug_start {"target": "day.rec", "replay": true}
debug {"session": "<session>", "command": "break till.to:25"}
=> "line":25
debug {"session": "<session>", "command": "continue", "wait": 30}
=> till.to:25
```

A replay answers what a debugger going forwards cannot. `timeline` gives every
value a binding took over the run, with the line that gave each: here, what
each of the three sales came to.

```mcp
debug {"session": "<session>", "command": "timeline amount"}
=> "value":"250"
=> "value":"0"
=> "value":"100"
```

The second sale counted nothing. `last` goes to the line after which a
condition last held, with the program as it was then, where `locals` shows
what it had.

```mcp
debug {"session": "<session>", "command": "last amount == 0"}
=> the last time `amount == 0` holds
debug {"session": "<session>", "command": "locals"}
=> cake,4.00
debug_stop {"session": "<session>"}
```

The till wrote `4.00`, which is no whole number, and `cents` made it 0 in
silence. An error that says which sale is wrong takes its place.

```edit till.to
-fn cents(sale: String): Int {
+fn cents(sale: String): Int ! String {
```

```edit till.to
-            err(_) => 0
+            err(_) => fail "`{sale}` gives no whole number of cents"
```

```edit till.to
-        none => 0
+        none => fail "`{sale}` has no comma"
```

```edit till.to
-fn main() {
+fn main(): !String {
```

```edit till.to
-                let amount = cents(sale)
+                let amount = cents(sale)?
```

```mcp
run {"path": "till.to", "stdin": "tea,250\ncake,4.00\nwater,100\n"}
=> `cake,4.00` gives no whole number of cents
```

## Two runs, one that went well

When a run that went well is recorded too, `debug_compare` says where the two
part: the first line that one ran and the other did not, and what differs
there. Both are runs of the same build.

```mcp
run {"path": "till.to", "stdin": "tea,250\ncake,400\nwater,100\n", "record": "good.rec"}
=> total 750
run {"path": "till.to", "stdin": "tea,250\ncake,4.00\nwater,100\n", "record": "bad.rec"}
debug_compare {"first": "good.rec", "second": "bad.rec"}
=> the runs part in task 1
=> "cake,400" → "cake,4.00"
```

## The other questions of a replay

- `why total` goes back to the line that gave a binding its value, and its
  `causes` say where each binding that line read got its own, to ask `why` of
  next; `origins total` follows them all the way to what the program read.
- `first total > 100` is `last`'s pair, and `ran till.to:20 total` gives each
  time a line ran.
- `back 3`, `forward` and `reverse` move through the lines, and `seek <probe>`
  goes to any moment the others named.

A test that fails only in CI is recorded the same way, with `--record` in
`flags`: `runbook-ci`. A failure that depends on the order of tasks is found
without a recording: `runbook-flaky`.
