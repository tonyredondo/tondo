# Performance

Making a program faster is three questions: is it slower than it was,
where does the time go, and what did the compiler make of that code. Tondo
answers each from the toolchain, and the third in the words of the source:
it says which calls it inlined and which it did not, what it put on the
heap and why, and which checks a loop keeps. Everything shown was run.

- [Is it slower: benchmarks](#is-it-slower-benchmarks)
- [Where the time goes: profiles](#where-the-time-goes-profiles)
- [What the compiler made: optimizations](#what-the-compiler-made-optimizations)
- [The machine code: asm](#the-machine-code-asm)
- [Against another version](#against-another-version)
- [Memory](#memory)
- [What not to be fooled by](#what-not-to-be-fooled-by)

## Is it slower: benchmarks

A benchmark is a test that calls `testing.bench`, and `tondo test --bench`
measures it: time, allocations, bytes and, on Linux, the instructions the
processor ran ([Testing](testing.md#benchmarks)).

```text
$ tondo test --bench --baseline before --max-regression 10
test words::unit::words::countingAPage ... ok
  count     447.7 µs/op  ±1.7%  9622 allocs/op  527160 B/op  6.67M ins/op  -42.3% faster, +0.0% ins, cycles -27.5%, l1d misses -46.6%
```

The habit that makes benchmarks useful is to judge by instructions.
Time on a shared machine moves by tens of percent with nothing changed, as
in the run above. The count of instructions barely moves from run to run,
so a change of 2% in it is a change in the program.

```text
tondo test --bench --against main --max-regression 2 --regression-by instructions
```

## Where the time goes: profiles

`tondo profile` samples a running program, or runs one from its start, and
`tondo test --bench --profile cpu` profiles a benchmark
([A running program](observing.md#where-the-time-goes-profile)).

```text
tondo profile -- ./report big.csv         a program, from its start
tondo profile api --cpu --duration 10     one that is running
tondo test --exact <id> --bench --profile cpu
```

`tondo profile show` reads the result three ways, each closer to the
metal:

| | |
| --- | --- |
| `tondo profile show cpu.pb.gz` | the functions that take the most |
| `tondo profile show cpu.pb.gz --lines` | the lines |
| `tondo profile show cpu.pb.gz --asm` | the instructions of the hottest function, each with its share of the samples |
| `tondo profile show cpu.pb.gz --html flame.html` | a flame graph, as one file |

By line, a profile reads like this:

```text
     flat  flat%       cum   cum%  line
     94ms  64.4%      94ms  64.4%  busy.to:14 in words  total += text.split(" ").length()
     45ms  30.8%      45ms  30.8%  busy.to:13 in words  let text = "word {i} and more words"
```

## What the compiler made: optimizations

A profile says which line is slow. `tondo optimizations` says what a
release build did with it. This is a small program that averages readings:

```tondo
import std.console

type Reading = { sensor: String, value: Float }

/// The mean of the readings of one sensor.
fn mean(readings: Array[Reading], sensor: String): Float {
    var total = 0.0
    var count = 0
    for reading in readings {
        if reading.sensor == sensor {
            total += reading.value
            count += 1
        }
    }
    if count == 0 { 0.0 } else { total / Float(count) }
}

fn label(sensor: String, value: Float): String {
    "{sensor}: {value:.1}"
}

fn main() {
    var readings: Array[Reading] = []
    for n in 0..1000 {
        readings.push(Reading { sensor: if n % 2 == 0 { "a" } else { "b" }, value: Float(n) })
    }
    console.println(label("a", mean(readings, "a")))
}
```

```text output
a: 499.0
```

```text
$ tondo optimizations stats.to
stats.to:9:9: `reading` is lent, with no count of its own: its array holds it (in main, mean)
stats.to:9:20: `readings` runs four turns at a time (in main, mean)
stats.to:12:13: `count += 1` cannot overflow: no check, in one copy of the code; another keeps the check (in main, mean)
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is copied (in main)
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is returned (in label)
stats.to:19:17: `value` stays a call of `formats.formatFloat`: its body costs 80, more than the 30 inlining takes (in main)
stats.to:19:17: `value` is inlined: the body of `formats.formatFloat` is copied here (in label)
stats.to:24:14: `0..1000` runs four turns at a time (in main)
stats.to:25:9: `readings.push(Reading { sensor: if n % 2 == 0 { "a" } else …` takes room for every turn before the loop (in main)
stats.to:25:44: `n % 2` is a shift or a mask: its dividend is not negative (in main)
stats.to:27:21: `label("a", mean(readings, "a"))` is inlined: the body of `label` is copied here (in main)
stats.to:27:32: `mean(readings, "a")` is inlined: the body of `mean` is copied here (in main)
```

Each line is a place of the program, what was done there, and why. Read
from the bottom:

- **Calls.** `mean` and `label` were inlined into `main`: their bodies are
  copied where they are called, and the call costs nothing. A call that is
  kept says what its body costs against the limit.
- **Arithmetic.** `n % 2` became a mask, since `n` is known not to be
  negative.
- **Loops.** Both loops run four turns at a time, and the array is given
  room for all thousand elements before the loop that fills it starts, so
  `push` never grows it.
- **The heap.** The label is a string that is returned, so it is made on
  the heap. A string, a closure or a record that does not outlive its
  function stays in the function's frame, and the line says which of the
  two happened and what decided it.
- **Checks.** The compiler proved that `count += 1` cannot overflow in one
  copy of the loop, which has no check; the copy it could not prove it for
  keeps it. A check that stays says so too.
- **Counts.** Each `reading` is a record with a string in it. The loop
  only reads it, so it is lent from the array, with no reference counted
  for the turn.

The same view says why not. This function keeps what it reads:

```tondo
import std.console

type Reading = { sensor: String, value: Float }

/// The readings of one sensor.
fn of(readings: Array[Reading], sensor: String): Array[Reading] {
    var found: Array[Reading] = []
    for reading in readings {
        if reading.sensor == sensor {
            found.push(reading)
        }
    }
    found
}

fn main() {
    var readings: Array[Reading] = []
    for n in 0..1000 {
        readings.push(Reading { sensor: if n % 2 == 0 { "a" } else { "b" }, value: Float(n) })
    }
    console.println("{of(readings, "a").length()}")
}
```

```text output
500
```

```text
$ tondo optimizations kept.to --function of
kept.to:8:9: `reading` keeps a count of its own: it is kept, as pushed, returned, captured or held (in of)
kept.to:8:20: `readings` runs one turn at a time: each turn counts a value or measures a text, which costs more than the test saved (in of)
kept.to:10:13: `found.push(reading)` grows as it is pushed to: the loop pushes to it in some turns only (in of)
```

Each remark names what the code does that the optimization could not get
past: the element is kept, so it is counted; counting makes a turn too
heavy to run four at a time; and the array cannot be given its room
beforehand, since only some turns push. None of it is wrong here, and the
view is how one knows that the cost is the program's and not an accident.

| | |
| --- | --- |
| `--function <name>` | one function |
| `--kind heap` | only what allocates |
| `--tests` | the tests and benchmarks instead of the program |
| `--against <rev>` | only what a change made different |
| `--json` | the same as data |

```text
$ tondo optimizations stats.to --kind heap
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is copied (in main)
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is returned (in label)
```

`tondo check --performance` reports one thing from this view as a warning
while writing: a copy that a loop makes on every turn.

## The machine code: asm

`tondo asm` gives the instructions a release build makes of a function,
under the lines they come from, with what `optimizations` says of each
line beside it:

```text
$ tondo asm stats.to mean
mean (1192 bytes, 12 of them constants)
      0  push rbp
      1  mov rbp, rsp
      4  sub rsp, 0x90
      ...
stats.to:9  for reading in readings {
        ; `reading` is lent, with no count of its own: its array holds it
        ; `readings` runs four turns at a time
     85  mov rdx, qword [r12 + rcx * 1 + 0x20]
     8a  mov qword [rsp], rdx
     8e  mov rcx, qword [r12 + rcx * 1 + 0x28]
     93  mov qword [rsp + 0x8], rcx
stats.to:10  if reading.sensor == sensor {
     98  mov rdi, qword [rsp]
     9c  mov rcx, qword [rip]  ; tondo_rt_str_eq
     a3  mov rsi, qword [rsp + 0x30]
     a8  call rcx
     aa  test al, al
     ac  jz $+0x19
     b2  movdqu xmm0, xmmword [rsp + 0x40]
stats.to:11  total += reading.value
     b8  vaddsd xmm0, xmm0, qword [r14 + 0x8]
     be  movdqu xmmword [rsp + 0x40], xmm0
```

What the remarks said is there to see. Under line 9 the element is read
where it lies in the array, with no call to count it. Line 10 is the one
call of the loop, the comparison of two strings, and line 11 is a single
addition. Nobody has to read assembly to use Tondo, but when a question
comes down to it, the answer is one command away and is laid out by the
source.

| | |
| --- | --- |
| `tondo asm <function>` | one function of the project |
| `tondo asm --all` | every function |
| `--clif` | Cranelift's intermediate code as well, before and after it optimizes |
| `--tests` | a function of the tests or benchmarks |
| `--against <rev>` | the difference with a revision's code |
| `tondo profile show <profile> --asm` | the same listing, with the share of the samples on each instruction |

## Against another version

Three commands compare with a revision of the project:

```text
tondo test --bench --against main            the benchmarks, measured in both
tondo optimizations --against main           what the compiler now does differently
tondo asm mean --against main                the machine code that changed
```

and `tondo test --bisect <commit> --bench --max-regression 5
--regression-by instructions` finds the commit where a benchmark started to
do more work.

`--compiler <tondo|rev>` in the same three commands compares what two
versions of Tondo itself make of the same program, which is how a change to
the compiler is judged.

## Memory

| | |
| --- | --- |
| `tondo test --bench` | allocations and bytes for each operation |
| `tondo optimizations --kind heap` | each place that allocates, and what keeps it on the heap |
| `tondo profile <program> --memory` | what is allocated and what is held, by line |
| `tondo test --exact <id> --bench --profile memory` | where a benchmark allocates |
| `tondo profile show <profile> --sample alloc_objects --lines` | the lines that allocate most often |
| `tondo dump diff a.json b.json` | what grew between two moments of a running program |
| `tondo counters <program>` | memory held, the rate of allocation, the collector's pauses |

A memory profile samples allocations. `TONDO_HEAP_RATE=1` in the program's
environment records every one, for a benchmark where each allocation is to
be accounted for.

Memory is counted, so most of it is freed the moment its last holder goes,
and there is no collector to tune. What a server holds follows what its
data structures hold.

## What not to be fooled by

- **Time on a busy machine.** Judge by instructions, and look at time
  last.
- **Where the code lies.** The same machine code at another address can
  run a few percent faster or slower, from how it meets the processor's
  caches and predictors. A change that moves code can look like a speed-up
  it did not cause. `tondo test --bench --layouts 5` measures each
  benchmark in five orders of the program's functions and gives the range,
  and only a difference beyond that range is real.
- **A benchmark that the compiler deletes.** A result that is not used may
  not be computed. `testing.bench` keeps what its closure gives, so the
  work that made it is not optimized away, and `testing.keep(value)` keeps
  a value the closure does not give.
- **The first run.** `tondo test --bench` warms up before it measures, and
  `--bench-time` says for how long it then does.

Next: [Editors and agents](editors-and-agents.md).
