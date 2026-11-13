# The toolchain

`tondo` is one executable. It compiles and links programs, and it is also
what tests them, debugs them, profiles them, and serves editors and coding
agents. There is nothing else to install and no versions to keep in step.

This page goes through what it does, with the commands and what they print.
`tondo help` lists every command, `tondo <command> --help` explains one, and
`tondo guide cli` has them all.

Each part has a chapter of its own, with the commands at work on real
programs and what they printed:

| Chapter | What it covers |
| --- | --- |
| [From a new project to an executable](toolchain/daily.md) | `new`, `check`, `fix`, `fmt`, `explain`, `api`, `run`, `build`, dependencies, `graph` |
| [Testing](toolchain/testing.md) | choosing tests, reading a failure, every order of a test's tasks, models, coverage, benchmarks, fuzzing, flaky tests, reports for CI, bisecting |
| [Debugging](toolchain/debugging.md) | a session, the commands, a recorded run debugged backwards, what a panic says, dumps and the tests they become |
| [A running program](toolchain/observing.md) | `ps`, `counters`, `requests`, `blocked`, logpoints, profiles, dumps of a live program, traces, live builds |
| [Performance](toolchain/performance.md) | benchmarks judged by instructions, profiles by line, what the optimizer did and why, the machine code |
| [Editors and agents](toolchain/editors-and-agents.md) | the language server, VS Code, other editors, the MCP server and its tools, runbooks, every command as JSON |
| [Every command](toolchain/commands.md) | each command with its options, as `tondo help --all` prints them |

- [Write and check](#write-and-check)
- [Run and build](#run-and-build)
- [Test](#test)
- [Every order of a test's tasks](#every-order-of-a-tests-tasks)
- [A design, checked in every state](#a-design-checked-in-every-state)
- [Benchmarks](#benchmarks)
- [Fuzzing](#fuzzing)
- [Debug, forwards and backwards](#debug-forwards-and-backwards)
- [A failure somewhere else: dumps](#a-failure-somewhere-else-dumps)
- [Watch a running program](#watch-a-running-program)
- [Change a running program](#change-a-running-program)
- [See what the compiler made](#see-what-the-compiler-made)
- [Editors](#editors)
- [Coding agents](#coding-agents)

The examples use this program, the `src/main.to` of a project named `words`:

```tondo
import std.console

/// How many times each word of `text` appears.
pub fn count(text: String): Map[String, Int] {
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        if word.isEmpty() {
            continue
        }
        counts[word] = (counts[word] ?? 0) + 1
    }
    counts
}

fn main() {
    console.println("{count("to be or not to be")}")
}
```

## Write and check

```text
tondo check                 diagnostics only, each with its code and what to write
tondo fix                   apply the repairs the compiler is sure of
tondo fmt src/main.to       the canonical format; --check changes nothing
tondo explain E1303         what a code of an error, a warning or a panic means
tondo api std.http          a module's declarations and docs, without bodies
tondo api --search base64   declarations by their names and docs
tondo guide tasks           one topic of the guide
```

An error says what is wrong and what to write instead:

```text
error[E1303]: a value of type `String ! fs.FsError` is not used; pass a failure on with `?`, handle it with `match`, or discard it with `_ = ...`
 --> unused.to:4:5
  |
4 |     fs.readText("x")
  |     ^^^^^^^^^^^^^^^^
```

`tondo check --json` gives the same as data, with spans and, for many, the
repair as edits. A warning that is accepted stays out of the list with a
comment that says why, `// tondo: allow W1004: the name C gives this type`,
and the compiler says when that comment stops being needed.

`tondo fmt` has no options of style, so every Tondo file looks the same.
`tondo api` is how a person or a model sees what a module offers: every type
of the standard library has an example there, and the tests compile them.

## Run and build

```text
tondo run -- --port 8080            compile, cache and run, with the program's arguments
tondo script.to a b                 the same for a script
tondo build --output app            a native executable
tondo build --static --strip        one with no dynamic libraries, without its symbols
tondo build --target aarch64-linux  for another architecture, from this machine
tondo build --cpu x86-64-v3         for processors with AVX2; x86-64-v2 or ARMv8.0 otherwise
tondo build --library               a library for C: lib<name>.so, lib<name>.a and a header
tondo bind lib.h --library z        the declarations of a C header, to call it
```

`tondo` links executables itself, ELF and Mach-O, so building needs no C
compiler and no development packages. `run` keeps what it builds by the
files it read, and a second run of the same code starts at once.

## Test

```tondo test
import std.testing

fn count(text: String): Map[String, Int] {
    var counts: Map[String, Int] = [:]
    for word in text.split(" ") {
        counts[word] = (counts[word] ?? 0) + 1
    }
    counts
}

test countsEachWord {
    assert(count("to be or not to be")["to"] == 2)
}

test counting {
    let text = "to be or not to be ".repeat(100)
    testing.bench("count 600 words", () { count(text) })?
}
```

Tests are in the language, beside the code they test, where they see what a
module keeps private, or in `tests/`, where they see only what is public.

```text
tondo test                       every test, in parallel worker processes
tondo test --filter count        those whose name has `count`
tondo test --watch               again as files change: the tests the change can affect
tondo test --affected main       only the tests that reach what changed since a commit
tondo test --coverage            lines, functions and branches that ran
tondo test --update-snapshots    record snapshots instead of checking them
tondo test --bisect v1.2 --filter count    the first commit where a test fails
tondo test --report junit=out.xml          also json, lcov, markdown and html
```

```text
test words::unit::words::counting ... ok
test words::unit::words::countsEachWord ... ok
ok. 2 passed; 0 failed; 0 skipped; 0 blocked; 0.00s

coverage:
  src/main.to  lines 83.3% (5/6)       functions 50.0% (1/2)     branches 100.0% (3/3)
  total        lines 83.3% (5/6)       functions 50.0% (1/2)     branches 100.0% (3/3)
```

A test that ends with tasks still running fails, and says where each one
waits and where it was spawned. `testing.withVirtualTime` runs code that
sleeps without waiting for the clock.

## Every order of a test's tasks

A bug between tasks shows only in some orders of them, so a test that
passes proves little. `tondo test` runs a test that starts tasks again in
every order of them that differs, and two orders differ only when two tasks
touch the same thing the other way around: a lock, an `Atomic`, a channel.

```tondo test failing
import std.sync

fn deposit(balance: sync.Mutex[Int], amount: Int) {
    let seen = balance.get()
    balance.set(seen + amount)
}

test depositsAddUp {
    let balance = sync.Mutex(0)
    let first = spawn deposit(balance, 10)
    let second = spawn deposit(balance, 5)
    first.wait()
    second.wait()
    assert(balance.get() == 15)
}
```

```text
test bank::unit::bank::depositsAddUp ... ok
bank::unit::bank::depositsAddUp: failed in order 1
again in that order: tondo test --exact bank::unit::bank::depositsAddUp --schedules at:0*15
debug it in that order: tondo debug --test bank::unit::bank::depositsAddUp --schedule at:0*15

panic[P0007]: assertion-failed: balance.get() == 15
  left:  5
  right: 15
```

The order it found can be run again and stepped through in the debugger.
With the read and the write in one step, `balance.update((value) { value +=
amount })`, the run says what it covered:

```text
note: 1 test that starts tasks passed in every order of them that differs
```

A plain run stops at 200 orders or 2 seconds a test. `--schedules all` goes
on to all of them, `--schedules 50` tries 50 at random, and nothing waits for
the clock in these runs: time jumps to the next timer, so a test with a
timeout is tried both ways. `--lock-order` fails a test whose tasks take two
locks in both orders, a deadlock that has not happened yet.

## A design, checked in every state

`std.model` goes through every state a piece of code, or a design that has no
code yet, can reach, and gives the steps to the state that breaks a rule. A
function that gives the next state for an event needs no model written for
it: the function is the model.

```tondo test failing
import std.model

enum Event {
    Insert
    Turn
    Push
}

type Gate = {
    paid: Bool
    open: Bool
}

fn apply(gate: Gate, event: Event): Gate ! String {
    match event {
        Insert => gate with { paid: true }
        Turn if gate.paid => gate with { open: true }
        Turn => fail "not paid"
        Push if gate.open => gate with { open: false }
        Push => fail "closed"
    }
}

test theGateOpensOnlyWhenPaid {
    let start = Gate { paid: false, open: false }
    let paid = model.always((gate: Gate) { not (gate.open and gate.paid) }, name: "never open and paid")
    model.expect(model.machine(start, apply, holds: [paid]))
}
```

Turning the gate should spend the coin, and this `apply` forgets to:

```text
panic[P0008]: explicit-panic: `never open and paid` does not hold
  start: Gate { paid: false, open: false }
  Insert: Gate { paid: true, open: false }
  Turn: Gate { paid: true, open: true }
```

Properties say what always holds, what eventually happens, what leads to
what, and anything else as a formula of temporal logic. `model.watch` holds
them to real concurrent code in every order of its tasks, and `model.follows`
keeps a model written by hand in step with the code it describes. `tondo
model apply` writes the test of a state machine and runs it. `tondo guide
models` goes through them.

## Benchmarks

A test that calls `testing.bench` is a benchmark. A plain `tondo test` runs
it once, so it cannot rot, and `--bench` measures it:

```text
tondo test --bench                       time, spread, allocations, bytes and instructions
tondo test --bench --save-baseline main  keep what was measured
tondo test --bench --baseline main       compare with it, with 95% confidence
tondo test --bench --against main --max-regression 5    fail a change 5% slower: a gate for CI
tondo test --bench --profile cpu         a profile of what each benchmark measured
tondo test --bisect v1.2 --filter count --bench --max-regression 5    the commit that made it slower
```

```text
  count 600 words     38.56 µs/op  ±6.7%  8 allocs/op  25712 B/op  445.5k ins/op
```

Time moves with what else the machine does and with where the code lies in
memory, so on Linux a benchmark also counts the instructions it ran, and
`--regression-by instructions` judges a change by them.

## Fuzzing

A test that calls `testing.fuzz` tries its code with inputs. The input can be
any type the program declares, not only bytes, and the search changes it
knowing its shape.

```tondo test
import std.json
import std.testing

test parsesAnyDocument {
    testing.fuzz("parse", ["{{}}", "[1, 2]", "\"x\""], (text: String) {
        match json.parse(text) {
            ok(value) => assert(json.parse(json.encode(value)) == ok(value))
            err(_) => {}
        }
    })
}
```

A plain `tondo test` runs the seeds and every input that failed before.
`tondo test --fuzz --fuzz-time 30` searches: it keeps the inputs that reach
code no input reached, takes values from the literals and comparisons of the
code, and when one fails it reduces it and saves it in `fuzz/`, as JSON, to
commit. From then on it is a test of regression.

## Debug, forwards and backwards

```text
tondo debug .                  break, step, look at every task
tondo debug --test <id>        one test
tondo debug api                a running Release build, by its name or process ID
tondo run --record rec .       record a run
tondo debug --replay rec       and debug it again, the same each time
tondo debug --compare good bad two recordings: where they part, and why
tondo dap                      the same debugger, for an editor
```

Breakpoints take conditions, hit counts and messages, and stop on what tasks
do as well as on lines: a spawn, a send, a lock, a request.

```text
(tondo) break src/main.to:10 if word == "or"
breakpoint 1 at src/main.to:10
(tondo) continue
task 1 stopped at src/main.to:10 in count (breakpoint 1)
   10 |         counts[word] = (counts[word] ?? 0) + 1
(tondo) locals
text: String = "to be or not to be"
counts: Map[String, Int] = ["to": 1, "be": 1]
word: String = "or"
```

A recorded run keeps its input, the files it read, its clock, its random
numbers and the order of its tasks, so it replays the same every time, on
another machine of the same system too. In a replay the debugger also goes
backwards, and `why` goes to the line that gave a binding its value, in
whichever function or task that was:

```text
(tondo) break src/main.to:12
(tondo) continue
task 1 stopped at src/main.to:12 in count (breakpoint 1)
   12 |     counts
(tondo) why counts
task 1 stopped at src/main.to:10 in count (`counts` became ["to": 2, "be": 2, "or": 1, "not": 1] on this line)
  `counts` was ["to": 2, "be": 1, "or": 1, "not": 1], from src/main.to:10 in count (probe 64)
  `word` was "be", from src/main.to:6 in count (probe 67)
   10 |         counts[word] = (counts[word] ?? 0) + 1
```

On Linux, `tondo test --record` keeps the runs of the tests that fail, which
is how a failure seen once in CI is debugged afterwards. For a service,
`--last 5m` keeps only the end of a long run.

A running Release build is debugged too, on Linux: a breakpoint stops the one
task that reaches it and the rest of the program goes on answering. A debug
build has DWARF, so gdb and lldb work as well, with a plugin that shows them
Tondo's tasks.

## A failure somewhere else: dumps

A program that panics, crashes or deadlocks leaves a dump: every task, where
it was and where it was spawned, the locks it held, the values of its frames,
and the live memory by where it was allocated. It costs nothing until the
program falls.

```text
tondo dump read api.dump why     what went wrong, as far as it can tell
tondo dump read api.dump stacks  the tasks, by the stack they share
tondo dump read api.dump locals  the bindings of a frame
tondo dump test api.dump         a test that fails as the dump did, with its values
tondo dump diff a.dump b.dump    what changed between two dumps
tondo dump group dumps/          the dumps by the failure they are, the most frequent first
tondo dump trend dumps/          what keeps growing over dumps: a leak, by its line
tondo dump issue dumps/ 8517     a failure written as an issue
tondo dump redact a.dump b.dump  one to share, without what the program held
tondo dump core core.123         a core of the system, read as a dump
```

`TONDO_DUMP_WHEN` makes a service dump by itself: every few minutes, near its
memory limit, when nothing moves, or when a request is slow. `@secret` on a
type or a field keeps its values out of dumps and the debugger.

A panic's own report already says what failed, with the values, and how the
program got there, through the task that spawned the one that failed:

```text
panic[P0001]: bounds: index 5 is out of bounds for an array of length 3: valid indices are 0 to 2, and -3 to -1 from the end
 --> src/main.to:6:5
stack:
  Cell.display       src/main.to:6:5
  depth              src/main.to:22:5  (3 times)
  closure in worker  src/main.to:29:17
  worker             src/main.to:29:5
in a task spawned at:
  main  src/main.to:38:15
```

## Watch a running program

A program started with `TONDO_DIAGNOSTICS=1` can be watched by its name or
its process ID. Without it, the program opens nothing and counts nothing.

```text
tondo ps                          the programs there are
tondo counters api --interval 1   memory, tasks, sockets, requests, and the program's own
tondo trace api --duration 5      what its tasks do, as a timeline for Perfetto
tondo profile api --cpu           where the time goes; --memory, --tasks
tondo profile show cpu.pb.gz      the functions and lines that take the most; --html a flame graph
tondo requests api                each HTTP and gRPC request, through every task it started
tondo blocked api                 who waits for whom
tondo logpoint api app.to:12 "order {order.id}"    what a line holds as it goes by
```

Profiles are in pprof's format, with the function and the line of every
frame, so the tools that read pprof read them:

```text
     flat  flat%       cum   cum%  line
     94ms  64.4%      94ms  64.4%  busy.to:14 in words  total += text.split(" ").length()
     45ms  30.8%      45ms  30.8%  busy.to:13 in words  let text = "word {i} and more words"
```

A logpoint prints what a line holds each time it runs, in a Release build
that is already running, with no new build and without stopping it; it works
on Linux. `ps`, `counters`, `trace`, `profile` and `dump` take
`--via "ssh host"` to reach a program on another machine.

## Change a running program

```text
tondo run --watch            reload as files are saved
tondo run --live             a live build, which `tondo reload <name>` changes
tondo debug --live .         edit where the program stopped, reload, continue
tondo eval app "total(order())"   what an expression gives in the running program
tondo values app src/main.to      what each line left the last time it ran
```

A live build takes in the code that was saved and keeps its state. Calls made
afterwards run the new code, a loop already running moves to its new body at
its next statement, and values of a type that gained or lost a field are
converted. Under `--watch` a panic does not end the program: the task waits,
and when a fix is saved the function that panicked starts again. Live builds
are for development, and a Release build has none of it.

## See what the compiler made

```text
tondo optimizations                  what a release build did with each place of the code
tondo optimizations --kind heap      only what allocates
tondo optimizations --against main   what a change made different
tondo asm count                      a function's machine code, under the lines it comes from
tondo check --performance            a copy a loop makes on every turn
tondo check --unreached              the code neither main nor a test reaches
tondo api --diff v1.2                what the public API lost, changed or gained since a commit
tondo graph --mermaid                the modules and what each imports
```

```text
src/main.to:5:36: `[:]` is made on the heap: a map (in count)
src/main.to:6:17: `text.split(" ")` stays a call of `String.split`: its body costs 74, more than the 30 inlining takes (in count)
src/main.to:10:24: `(counts[word] ?? 0) + 1` keeps its overflow check, in a loop (in count)
src/main.to:16:23: `count("to be or not to be")` stays a call of `count`: its body costs 35, more than the 30 inlining takes (in main)
```

Each line says what was decided and why, so the reason a loop is slow or a
value allocates is read rather than guessed.

## Editors

`tondo lsp` is a language server for any editor: diagnostics with their
fixes as you type, completion that writes the import, navigation, rename,
refactors that are checked by compiling, inlay hints and formatting.

The VS Code extension, in [`editors/vscode`](../editors/vscode), adds what
the rest of this page describes: running and debugging with F5, the tests in
the Testing view with coverage, *Test in Every Order*, live builds that take
in what is saved, profiles beside the code, dumps and recordings opened as
debugging sessions, and the values a running program left beside its lines.
`tondo setup` installs it in VS Code, Cursor, VSCodium and Windsurf.

## Coding agents

Tondo is written to be easy for a model to write, and its tools are made to
be driven by one.

- `tondo guide` teaches the language a topic at a time, with examples that
  compile, and `tondo guide pitfalls` lists the mistakes that habits from
  other languages cause, with the error each gives.
- `tondo api` shows a module without its bodies, in about a quarter of the
  tokens, and `tondo fmt --compact` prints a file in about 15% fewer.
- Every command takes `--json` and writes one object with its result, and
  every diagnostic has a code, a span and, when the compiler is sure, the
  edit that fixes it.
- `tondo mcp` is an MCP server whose tools do all of the above: check and
  fix, navigate, run with input and a timeout, test, debug, profile and read
  dumps. `tondo guide runbooks` says what to do, step by step, for a crash,
  a hang, a flaky test or a slow request.
- `tondo setup` gives Claude Code and Codex the Tondo skill and the server.
