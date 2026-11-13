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
| `tasks`, `task 2`, `blocked` | every task, also those that wait; one of them; who waits for whom |
| `set count = 3` | changes a binding |
| `snapshot` | everything about a stop at once: why, every frame with its bindings, the other tasks, the last lines run |

`locals` gives more than the function's bindings: also what a closure
captured, and the bindings of the functions it is written in that are on
the stack.

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
task 1 stopped at src/main.to:10 in count (1 lines back)
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

## A failure somewhere else: dumps

The last line of that report matters when the program is not on the
developer's machine. A program that panics, faults or deadlocks leaves a
dump, with the values each task held when it ended. `TONDO_DUMP` says
where; `~/.cache/tondo/dumps` keeps the latest by default. The dump is a
file to attach to an issue.

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
| `tondo dump diff a.dump b.dump` | what changed between two dumps |
| `tondo dump group <dir>` | the dumps by the failure they are, the most frequent first |
| `tondo dump trend <dir>` | what keeps growing over dumps: a leak, by its line |
| `tondo dump issue <dir> <id>` | a failure written as an issue |
| `tondo dump <pid>` | a dump of a program that is running, without stopping it |

A field marked `@secret` shows as `<secret>` in dumps and in the debugger.

## A program that is running

`tondo debug <pid>` attaches to a debug build that is running, and
`--via` reaches one on another machine or in a container, where `tondo` is
installed too:

```text
tondo debug --via "ssh api-3" api
tondo debug --via "docker exec -i box" 4721
```

A live build, `tondo run --live`, goes one step further: at a stop,
`reload` changes the program's code to what is in the files now, and the
session goes on with its state. A breakpoint's condition there is compiled,
and may call the program's functions.

For a program that should not be stopped at all, the next chapter:
[A running program](observing.md).
