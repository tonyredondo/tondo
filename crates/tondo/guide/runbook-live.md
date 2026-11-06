# Runbook: changing a running program

Try a change without starting the program again and without losing what it
holds: a server's connections, the state a bug took minutes to reach. This is
for development; a build for production is a normal one.

```tondo file=app.to
import std.console
import std.time

fn price(units: Int): Int {
    units * 2
}

fn main() {
    var units = 0
    for {
        units += 1
        console.println("{price(units)}")
        time.sleep(time.milliseconds(100))
    }
}
```

`run_start` runs it live: its code can change while it runs.

```mcp
run_start {"path": "app.to", "wait": 2}
run_output {"session": "<session>", "wait": 10}
=> 2
```

`eval` asks the program, without stopping it: what a function gives, as its
files say now. It is the fastest way to try a change. It takes in what is
saved, so the program runs that code from then on.

```mcp
eval {"program": "<pid>", "expression": "price(4)"}
=> "value":"8"
```

Save a change, and `reload` puts it in. It says what changed, and what could not
change at once: a frame on old code moves at its next statement.

```edit app.to
-    units * 2
+    units * 10
```

```mcp
reload {"session": "<session>"}
=> 1 changed (price)
eval {"program": "<pid>", "expression": "price(4)"}
=> "value":"40"
```

## A panic in a live program

It does not end the program: the task waits there for a fix, with all it held.

```edit app.to
-    units * 10
+    units / (units - units)
```

```mcp
reload {"session": "<session>"}
=> 1 changed (price)
run_output {"session": "<session>", "wait": 10}
=> task 1 waits here: save a fix
```

`values` shows what waits and what it held, and `eval` sees the bindings of its
frame, so the fix can be written from what is known.

```mcp
values {"program": "<pid>", "file": "app.to"}
=> integer-division-by-zero
eval {"program": "<pid>", "expression": "units"}
=> "value":
```

Fix it and `reload`: the function starts its statement again, and the program
goes on as if nothing had happened.

```edit app.to
-    units / (units - units)
+    units * 3
```

```mcp
reload {"session": "<session>"}
=> that panicked starts its statement again
run_stop {"session": "<session>"}
```

With `watch` in `run_start` it reloads on each save by itself. A change that
cannot go in, as a type of a field a live value has, says so and `restart` is
in its result: then start it again.
