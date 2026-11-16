# Runbook: debugging a program

For when the failure does not say enough: to see what a program holds as it
runs. Prefer `run`, `test` and `dump_read` when they answer; a debugger is for a
value that is wrong with no panic to say where.

```tondo file=tax.to
import std.console

fn rate(country: String): Int {
    if country == "es" {
        return 21
    }
    0
}

fn total(amount: Int, country: String): Int {
    let tax = amount * rate(country) / 100
    amount + tax
}

fn main() {
    console.println("{total(100, "ES")}")
}
```

It prints 100, and 121 was expected. `debug_start` builds it for debugging and
stops before `main`; `debug` takes one command at a time, as `tondo debug` does.

```mcp
debug_start {"target": "tax.to"}
debug {"session": "<session>", "command": "break tax.to:12"}
=> "line":12
debug {"session": "<session>", "command": "continue", "wait": 30}
=> tax.to:12
```

`locals` shows what the code there can use, and `print` any expression of it.
`why` goes back to the line that gave a value its value, which is usually
quicker than stepping towards it.

```mcp
debug {"session": "<session>", "command": "locals"}
=> tax
debug {"session": "<session>", "command": "print country"}
=> ES
debug_stop {"session": "<session>"}
```

The country is upper case, and `rate` compares with lower case.

```edit tax.to
-    if country == "es" {
+    if country.toLower() == "es" {
```

```mcp
run {"path": "tax.to"}
=> 121
```

A program that runs in production is a Release build: `debug_start` on its
process, with `TONDO_DIAGNOSTICS=1`, stops one task while the others go on and
costs nothing until then. `logpoint` shows what a line holds, without stopping
anything, through a `nop` the build leaves at the start of each line, which
a build with `--no-logpoints`, or `logpoints = false` in the `[build]` of its
`tondo.toml`, leaves out, and then takes no logpoint: `runbook-running`. `catch panic` stops where a panic starts, with all its
frames. A run that cannot be had again is recorded and debugged backwards:
`runbook-once`.

A result that the release build gets wrong and the debug build right, in a
program that depends on no task's order and on no time, is a bug of tondo's,
and regions are what only the release build has. `TONDO_CHECK_REGIONS=1` in
the program's environment makes a value that outlived its region fault where
it is read, `fatal: SIGSEGV … in the memory of a region that ended: the
compiler put a value in a region it outlives, a bug of tondo's`, with the
line. `no_regions` in `run`, `test` and `build`, `--no-regions` in the
commands, builds without regions: a result that is right again was theirs,
and that build is the one to go on with while the bug is reported.
