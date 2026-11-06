# Runbook: memory that grows

A program that keeps growing keeps what it should let go. Dumps taken over time
say what grows and from which line: `TONDO_DUMP_WHEN` takes them by itself.

```tondo file=keeps.to
import std.console
import std.time

fn remember(seen: var Array[String], round: Int) {
    for at in 0..400 {
        seen.push("request {round} {at}, with what a cache that never lets go holds")
    }
}

fn main() {
    var seen: Array[String] = []
    for round in 0..300 {
        remember(var seen, round)
        time.sleep(time.milliseconds(10))
    }
    console.println("{seen.length()}")
}
```

```sh
tondo build keeps.to --output keeps
mkdir dumps
TONDO_DUMP=dumps TONDO_DUMP_WHEN=every=200ms,keep=100 ./keeps
```

`dump_trend` compares them: what keeps growing, by the line that made it, how
fast, and which requests grow slower and where tasks pile up.

```mcp
dump_trend {"dir": "dumps"}
=> keeps.to:6
```

`dump_diff` compares two dumps, by place, and `dump_read` with `heap` says what
one holds. In a debugger, `retainers <path>` says what keeps a value alive.

Line 6 is the array that only grows: keep what is needed, or a fixed number of
the last ones, and the trend goes flat.
