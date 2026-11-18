# Debugging

`tondo debug` is a debugger for Tondo programs that speaks Tondo: its
values are printed as the language writes them, its conditions are
expressions of the language, and it knows about tasks, channels and
results. It runs in a terminal, behind an editor, and for a coding agent,
and it can go backwards. The sessions on this page were run, on the small
program of [the daily loop](daily.md), and are shown as they went.

- [A session](#a-session)
- [The commands](#the-commands)
- [A test, in the order that fails](#a-test-in-the-order-that-fails)
- [Backwards: a recorded run](#backwards-a-recorded-run)
- [What a panic says](#what-a-panic-says)
- [A release build that gives another result](#a-release-build-that-gives-another-result)
- [A failure somewhere else: dumps](#a-failure-somewhere-else-dumps)
- [A program that is running](#a-program-that-is-running)

## A session

The program counts the words of a file. A breakpoint with a condition
stops it where a particular word is counted:

```text
$ tondo debug .
the program waits before main: set breakpoints, then `continue`
> break src/main.to:11 if word == "not"
breakpoint 1 at src/main.to:11
> continue
task 1 stopped at src/main.to:11 in count (breakpoint 1)
   11 |         counts[word, default: 0] += 1
> locals
text: String = "To be, or not to be: that is the question.\n"
counts: Map[String, Int] = ["to": 1, "be": 1, "or": 1]
each: String = "not"
word: String = "not"
> print counts.length()
counts.length(): Int = 3
> stack
#0 src/main.to:11 in count
#1 src/main.to:19 in main
> next
task 1 stopped at src/main.to:9 in count (step)
    9 |     for each in text.words() {
> print counts
counts: Map[String, Int] = ["to": 1, "be": 1, "or": 1, "not": 1]
```

- `tondo debug` takes a file, a project, an executable built with
  `--debug`, or a running debug build by its process ID or name. It builds
  what it needs.
- Values are shown with their types, as literals of the language. A map is
  a map, an option is `some(3)` or `none`, and a record is its fields: there
  are no pointers to follow and no internals of a collection to read.
- A condition is an expression: bindings and their parts, literals,
  arithmetic, comparisons, `and`, `or`, `not`, and methods that only read,
  as `length()`, `isEmpty()`, `contains()` and `startsWith()`. Conditions
  and `print` read values and never run the program's code.
- Steps go over the standard library's code unless `steps all` says
  otherwise, and a step that leaves a function says what it returned.

## The commands

```text
break src/main.to:12 [if count > 3 and name != ""] [hits 5] [log at {i}: {total / i}]
break fn parse   watch total   delete <id>   catch panic bounds error ConfigError
continue   step   next   finish   steps all   pause   wait
record   back 3   forward   reverse   record off
locals   print items[i + 1].name   print total / count   children items 100 50
set count count + 1   set user.age = 37   context   stack   frame 1   task 2
tasks   blocked   cancel 2   reload   restart   goto 12   call total(items)
break on spawn|send|receive|lock|request|recover|wait 100
```

| | |
| --- | --- |
| `break <file>:<line>` | a breakpoint, with a condition (`if`), a count (`hits 5`), or a message instead of a stop (`log`) |
| `break fn <name>` | where a function begins |
| `break on spawn`, `send`, `receive`, `lock`, `request` | where the program does a kind of thing |
| `catch panic`, `catch error <Type>` | where a panic starts; where an error starts, a `fail` or a `?` |
| `watch <binding>` | stops when a value changes |
| `step`, `next`, `finish`, `until 30` | into, over, out, and to a line |
| `locals`, `print <expression>`, `children items 100 50` | what the line can use; one value; a page of a large one |
| `context` | the program's contexts, and the `with` that set each field |
| `stack`, `frame 1` | the calls, and looking at another one |
| `tasks`, `task 2`, `blocked` | every task, also those that wait or are inside a call of C; one of them; who waits for whom |
| `set count = 3` | changes a binding |
| `snapshot` | everything about a stop at once: why, every frame with its bindings, the other tasks, the last lines run |

`locals` gives more than the function's bindings: also what a closure
captured, and the bindings of the functions it is written in that are on
the stack.

`tasks` answers once the stop holds every task: each is at a line, waits,
or is inside a call of C, where it stands as still as one that waits until
the call returns. It is listed where it called C, and `stack` shows its
frames from there:

```text
> tasks
task 1 at napped.to:16 in main (breakpoint 1)
task 2 at napped.to:8 in napper (in a call of C)
> stack 2
#0 napped.to:8 in napper
   spawned at napped.to:14 in main
```

A logpoint needs no stop and no rebuild. `tondo logpoint <name>
src/main.to:11 "{word}: {counts.length()}"` writes a line each time a
running program passes there.

## A test, in the order that fails

When `tondo test` finds an order of tasks in which a test fails, it prints
the command that debugs it in that order
([Testing](testing.md#tests-that-start-tasks)):

```text
debug it in that order: tondo debug --test bank::unit::bank::depositsAddUp --schedule at:0*15
```

The order is fixed, so the bug is there every time. `tasks` shows what
each task was doing, and `break on lock` or `break on send` stops where the
tasks meet.

`tondo debug --test <id>` on a fuzz test that failed stops where it fails,
with the input that broke it.

## Backwards: a recorded run

`tondo run --record` keeps a run: what the program read, the time it saw,
and the order of its tasks. The recording is a directory, to copy
anywhere.

```text
$ tondo run --record run.rec
to 2
be 2
...
tondo: the run is recorded in run.rec: `tondo debug --replay run.rec` runs it again
```

`tondo debug --replay` runs it again, the same each time, and can ask the
questions that a debugger going forwards cannot. Here the map is looked at
when `count` returns, and then asked where it came from:

```text
$ tondo debug --replay run.rec
> break src/main.to:13
breakpoint 1 at src/main.to:13
> continue
task 1 stopped at src/main.to:13 in count (breakpoint 1)
   13 |     counts
> print counts
counts: Map[String, Int] = ["to": 2, "be": 2, "or": 1, "not": 1, "that": 1, "is": 1, "the": 1, "question": 1]
> why counts
task 1 stopped at src/main.to:11 in count (`counts` became ["to": 2, "be": 2, "or": 1, "not": 1, "that": 1, "is": 1, "the": 1, "question": 1] on this line)
  `counts` was ["to": 2, "be": 2, "or": 1, "not": 1, "that": 1, "is": 1, "the": 1], from src/main.to:11 in count (probe 1059)
  `word` was "question", from textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 1320)
   11 |         counts[word, default: 0] += 1
```

`why counts` went back to the line that gave `counts` its value, and its
causes say where each binding that line read got its own. `counts` got it
on this same line, a turn of the loop before. `word` came from a call, so
its cause is the line that computed it, inside the function that was
called. Asking `why` of one of them goes a step further back. The program
is as it was at that moment, so `locals` and `print` work there, and `back`
goes on from it:

```text
> locals
text: String = "To be, or not to be: that is the question.\n"
counts: Map[String, Int] = ["to": 2, "be": 2, "or": 1, "not": 1, "that": 1, "is": 1, "the": 1]
each: String = "question."
word: String = "question"
> timeline word
"to"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 74)
"be"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 202)
"or"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 295)
"not"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 423)
"to"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 523)
"be"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 649)
"that"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 796)
"is"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 896)
"the"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 1024)
"question"  at textutil/src/clean/clean.to:3 in closure in textutil.clean.word (probe 1320)
> back 1
task 1 stopped at src/main.to:10 in count (1 line back)
   10 |         let word = clean.word(each)
> first counts.length() > 2
task 1 stopped at src/main.to:11 in count (the first time `counts.length() > 2` holds, after this line)
   11 |         counts[word, default: 0] += 1
```

| | |
| --- | --- |
| `back 3`, `forward`, `reverse` | lines back, forward again, and running backwards to a breakpoint |
| `why <binding>` | the line that gave it its value, with where each thing that line read came from |
| `origins <binding>` | `why`, followed all the way back to what the program read |
| `timeline <binding>` | every value it took, and the line that gave each |
| `first <condition>`, `last <condition>` | the line after which it first, or last, held |
| `ran src/main.to:12 total` | each time a line ran, with a value |
| `seek <probe>` | goes to any of the moments the others named |
| `diff 120` | what changed since a probe, task by task |

`tondo run --record run.rec --last 5m` keeps only the end of a run, for a
server whose failure comes after hours, and `tondo debug --compare a.rec
b.rec` takes two recordings of the same build, as one that passed and one
that failed, and says where they part.

In a session that is not a replay, `record` starts keeping each line the
tasks run, with its bindings, and `back` and `forward` go through them
while the program is stopped.

## What a panic says

A bug stops a program with a panic, and the report is meant to be enough
on its own: a code, the words, the place, and the calls that led there.

```text
$ ./orders
panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
 --> orders.to:8:16
stack:
  total  orders.to:8:16
  main   orders.to:15:23
tondo runtime: a dump of the program is in dumps/orders-3573589.dump.json
```

`tondo explain P0001` says what the code means and what usually causes it.
A release build reports the same, with the same lines: the checks are not
a feature of debug builds. With `TONDO_PANIC_FORMAT=json` in
its environment a program writes the report as JSON, for a log pipeline.

## A release build that gives another result

A debug build and a release build are the same program: what one prints
the other prints, unless the program depends on the time or on the order
its tasks run in. When only the release build gives a wrong result, the
cause is the compiler, and one thing that only a release build has is
regions: the memory of a scope, let go of at once when the scope ends
([Performance](performance.md#what-a-region-takes)). A value that the
compiler put in a region it outlives does not fail. It reads what the next
region wrote in its place, which is often a value of the same shape, and
the program goes on with data that looks right.

`TONDO_CHECK_REGIONS=1` in the program's environment makes it fail. The
memory of each region that ends becomes unreadable and its addresses are
not used again, so the first read of such a value is a fault, and the
report says what it is and the line that read:

```text
$ TONDO_CHECK_REGIONS=1 ./slices
fatal: SIGSEGV: invalid memory access at 0x7e5839008238, in the memory of a region that ended: the compiler put a value in a region it outlives, a bug of tondo's
stack:
  main  slices.to:9:21
```

That report is of a bug mended since, in a loop that assigned to a slice
of an array. The check is the runtime's, so the release executable that
gave the wrong result takes it as it is, with nothing built again. `tondo
test` gives the variable to the tests it runs, as it gives them the rest
of its environment, and a build gives it to the programs it runs to
compile, the derive providers and the generators of
[Metaprogramming](../language/17-meta.md), which otherwise start with an
empty one.

Its price is by region that ends: a page of memory that is new, and two
calls to the system. A program that builds a tree of thousands of nodes in
each region ran twice as slow with it. A loop of three million turns that
makes an array of three numbers in each went from 0.02 s to 4 s. It is a
check for the run that shows the wrong result, not one to leave on.

`--no-regions` builds the program without regions, every value on the
heap as a debug build has it, and `regions = false` under `[build]` in
`tondo.toml` does so for every build of the project:

```text
$ tondo build --no-regions
$ tondo test --no-regions
```

A wrong result that goes away without them is the regions', whether or not
the check found the read. It is also the build to go on with meanwhile: it
is slower only where regions were faster
([Performance](performance.md#what-a-region-takes)). Either way the bug is
tondo's, to report with the program or the smallest piece of it that shows
the result.

## A failure somewhere else: dumps

The last line of that report matters when the program is not on the
developer's machine. A program that panics, faults or deadlocks leaves a
dump, with the values each task held when it ended. `TONDO_DUMP` says
where, a directory or `stderr` for the logs of a
[container](observing.md#in-a-container) or of CI; `~/.cache/tondo/dumps`
keeps the latest by default. The dump is a file to attach to an issue.

```text
$ tondo dump read dumps/orders-3573589.dump.json why
it stopped at a panic
  panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
   --> orders.to:8:16
what came last:
     0 us ago  task 1 panicked: panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end at orders.to:8:16

$ tondo dump read dumps/orders-3573589.dump.json locals
orders.to:8 in total
order: Order = Order { id: 7, lines: [10, 20] }
```

The values are there, so the failure can be made into a test. `tondo dump
test` writes it:

```text
$ tondo dump test dumps/orders-3573589.dump.json
// From a dump: panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
test failedInTotal {
    let order = Order { id: 7, lines: [10, 20] }
    _ = total(order)
}
```

That test fails as production did, on the developer's machine, before any
fix is written, and passes once the bug is gone. With `tondo test --bisect`
it also finds the commit that brought the bug.

What a dump holds depends on the build. One made with `--debug` keeps every
value of every frame. A Release build keeps none on purpose, and its dump
has what is still in place when the task stops: at a check that fails, of
bounds, of an overflow or of a division by zero, the values that the
function's code still held. This function fails in its third turn:

```tondo
import std.console

type Order = { id: Int, lines: Array[Int] }

/// What the order comes to, with a discount on each line.
fn total(order: Order, discount: Int): Int {
    var sum = 0
    let count = order.lines.length()
    for index in 0..=count {
        let line = order.lines[index]
        sum += line - discount
    }
    sum
}

fn main() {
    let order = Order { id: 7, lines: [10, 20] }
    console.println("{total(order, 1)}")
}
```

```text
$ tondo build bill.to -o bill
$ TONDO_DUMP=dumps ./bill
panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
 --> bill.to:10:20
stack:
  total  bill.to:10:20
  main   bill.to:18:23
tondo runtime: a dump of the program is in dumps/bill-138607.dump.json
$ tondo dump read dumps/bill-138607.dump.json locals
bill.to:10 in total
order: Order = Order { id: 7, lines: [10, 20] }
discount: Int = 1
sum: Int = 28
index: Int = 2
```

`count` and `line` are not there: by that point the code had let go of
them. Which values it still holds is a matter of the machine code, so an
arm64 build of this program may keep others than this one, of x86-64; what
a dump shows is right on both. A value that is always the same constant,
as `discount` here, is shown though the code keeps it nowhere. Built with
`--debug`, the same failure leaves them all:

```text
$ tondo build bill.to --debug -o bill
$ TONDO_DUMP=dumps-debug ./bill
...
$ tondo dump read dumps-debug/bill-138655.dump.json locals
bill.to:10 in total
order: Order = Order { id: 7, lines: [10, 20] }
discount: Int = 1
sum: Int = 28
count: Int = 2
index: Int = 2
line: Int = 20
```

The values of the Release dump cost nothing while the program runs: its
code is the same, and the dump reads what is still where the code left it.
This one holds both arguments of `total`, so `tondo dump test` writes the
test from it too; a tool that needs a value the dump lacks says which, and
that a build with `--debug` keeps them all.

A service leaves many dumps, most of them of the same few failures:

```text
$ tondo dump group dumps
1 x 82a9ef60757d panic
  panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
  at orders.to:8 in total
  the last: dumps/orders-3573589.dump.json
```

| | |
| --- | --- |
| `tondo dump read <dump> why` | what went wrong, as far as it can tell |
| `tondo dump read <dump> stacks` | the tasks, by the stack they share |
| `tondo dump read <dump> locals` | the bindings of a frame |
| `tondo dump test <dump>` | a test that fails as the dump did |
| `tondo dump test <dump> --bench` | a benchmark of the call a frame was in, with its values ([Performance](performance.md#is-it-slower-benchmarks)) |
| `tondo dump diff a.dump b.dump` | what changed between two dumps |
| `tondo dump group <dir>` | the dumps by the failure they are, the most frequent first |
| `tondo dump trend <dir>` | what keeps growing over dumps: a leak, by its line |
| `tondo dump issue <dir> <id>` | a failure written as an issue |
| `tondo dump <pid>` | a dump of a program that is running, without stopping it |

`why` and `tasks` also name a task that ended with an error nobody had
read, which is often what the tasks that wait were waiting for
([Deadlocks](../language/13-tasks.md#deadlocks)).

A field marked `@secret` shows as `<secret>` in dumps and in the debugger.

## A program that is running

`tondo debug <pid>` attaches to a debug build that is running, and
`--via` reaches one on another machine or in a container, where `tondo` is
installed too:

```text
tondo debug --via "ssh api-3" api
tondo debug --via "docker exec -i box" 4721
```

A program in a container whose image has no `tondo` is debugged from a
container beside it: [A running program](observing.md#in-a-container) has
the commands.

A live build, `tondo run --live`, goes one step further: at a stop,
`reload` changes the program's code to what is in the files now, and the
session goes on with its state. A breakpoint's condition there is compiled,
and may call the program's functions.

For a program that should not be stopped at all, the next chapter:
[A running program](observing.md).
