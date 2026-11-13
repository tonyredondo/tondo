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
- [The processor a build is for](#the-processor-a-build-is-for)
- [Many numbers at once: vectors](#many-numbers-at-once-vectors)
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
mean (1224 bytes, 12 of them constants)
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
     b8  addsd xmm0, qword [r14 + 0x8]
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
| `--cpu <level>` | the code of a [level of processor](#the-processor-a-build-is-for) |
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

## The processor a build is for

A build is for a level of processor, which says which instructions its code
may use. The default is `baseline`, x86-64-v2 on x64 and ARMv8.0 on arm64,
whichever machine builds, so an executable made on a new machine runs on an
older one. `--cpu` asks for another level in `build`, `run` and `test`, and
so does `cpu` in the `[build]` of the program's `tondo.toml`:

| Level | For | Its code may use |
| --- | --- | --- |
| `baseline` | x64 | SSE3, SSSE3, SSE4.1, SSE4.2, POPCNT and CMPXCHG16B, which is x86-64-v2 |
| `x86-64-v3` | x64 | those, and AVX, AVX2, FMA, BMI1, BMI2 and LZCNT |
| `x86-64-v4` | x64 | those, and AVX-512 F, DQ and VL |
| `baseline` | arm64 | ARMv8.0 with NEON, which every 64-bit ARM has |
| `native` | both | what the processor of the machine that builds has |

What a level gains is measured as anything else is: the benchmarks at one
level, kept, and again at the other. This function asks for the leading
zeros of each word, which x86-64-v3 has an instruction for:

```tondo
import std.console

/// How many bits the largest of the words needs.
pub fn widest(words: Array[Int]): Int {
    var most = 0
    for word in words {
        most = most.max(64 - word.leadingZeros())
    }
    most
}

fn main() {
    console.println("{widest([5, 300])}")
}
```

```text output
9
```

```text
$ tondo test --bench --save-baseline level
test sizes::unit::sizes::widestOf10000 ... ok
  widest     11.18 µs/op  ±0.2%  0 allocs/op  0 B/op  125.1k ins/op
$ tondo test --bench --cpu x86-64-v3 --baseline level
test sizes::unit::sizes::widestOf10000 ... ok
  widest     4.103 µs/op  ±0.1%  0 allocs/op  0 B/op  85.1k ins/op  -63.3% faster, -32.0% ins, cycles -63.7%
```

`tondo asm widest` shows why, under each turn of the loop. At `baseline`
the line is a `bsr` and what makes it right for a word of zero:

```text
src/main.to:7  most = most.max(64 - word.leadingZeros())
        ; `64 - word.leadingZeros()` keeps its overflow check, in a loop
     6f  mov r9, -0x1
     76  bsr rsi, rdx
     7a  cmovz rsi, r9
     7e  mov edx, 0x3f
     83  sub rdx, rsi
     86  sub r11, rdx
     89  jo $+0x382
     8f  lea rdx, qword [rcx + 0x1]
```

`tondo asm widest --cpu x86-64-v3` has one `lzcnt` there. `tondo
optimizations` takes the option too:

```text
src/main.to:7  most = most.max(64 - word.leadingZeros())
        ; `64 - word.leadingZeros()` keeps its overflow check, in a loop
     76  lzcnt r8, r8
     7b  sub rsi, r8
     7e  jo $+0x2f9
     84  lea r8, qword [rdx + 0x1]
```

Code that uses nothing the level adds is the same at both, and that is
most code: [`bench/README.md`](../../bench/README.md) has every benchmark
of Tondo at the two levels.

A program checks its processor when it starts, before any of its own code
runs. On one that lacks what the level needs it says so, and which build
runs there, where it would otherwise stop at the first instruction the
processor does not have:

```text
$ tondo build --cpu x86-64-v4
$ ./sizes
fatal: this program was built for x86-64-v4 and this processor lacks AVX-512F, AVX-512DQ and AVX-512VL: build it with --cpu baseline
$ echo $?
101
```

`env.build().cpu` gives the level a program was built for, and
[`std.cpu`](../std/cpu.md) what its processor has beyond it:

```tondo
import std.console
import std.cpu
import std.env

fn main() {
    console.println("built for {env.build().cpu}")
    if cpu.has(cpu.Feature.Avx2) {
        console.println("this processor has AVX2")
    } else {
        console.println("this processor has no AVX2")
    }
}
```

Built with no option, on the machine of this page:

```text
built for baseline
this processor has AVX2
```

With the variant written in the call, the compiler answers `cpu.has` when
it can: `true` for a feature of the build's level, and `false` for one of
the other architecture. Only the way of the `if` that runs is compiled
then: built with `--cpu x86-64-v3`, the program above has neither the test
nor the second message in it. Any other feature is read from what the
program found when it started, and `cpu.features()` gives all of them.

`std.cpu.x64` and `std.cpu.arm64` have functions that are one instruction
each, for the files of their architecture
([Modules](../language/15-modules.md#code-for-one-system-or-architecture)):
CRC32C, the rounds of AES, carry-less multiplication, and BMI2's `pdep` and
`pext`. A function that uses one needs its feature. The compiler works that
out from the bodies, for the whole program, and nothing is written for it:
the call goes under a test of the processor, with the plain way beside it.

```text
// src/bits/bits_x64.to
import std.cpu
import std.cpu.x64

fn deposit(value: UInt, mask: UInt): UInt {
    if cpu.has(cpu.Feature.Bmi2) { byInstruction(value, mask) } else { byLoop(value, mask) }
}

/// BMI2's `pdep`, one instruction.
fn byInstruction(value: UInt, mask: UInt): UInt {
    x64.depositBits(value, mask)
}
```

`byInstruction` is compiled for BMI2 and the rest for the build's level, so
the instruction runs only where the processor has it. `tondo optimizations`
says why that call is kept:

```text
src/bits/bits_x64.to:5:36: `byInstruction(value, mask)` stays a call of `bits.byInstruction`: it needs features of the processor this function does not (toolchain 7.8) (in bits.deposit, bits.spread, main)
```

Without the test, every function up to `main` would need BMI2, and `main`
cannot need a feature; nor can a test, a `pub extern fn`, a trait's method
or a function used as a value. The error names the calls that bring it:

```text
error[E1809]: this call needs `Bmi2` of the processor, and `main`, or the top level of a script, cannot need one: `main` calls `bits.spread`, which calls `bits.deposit`, which uses `x64.depositBits`; put the call under `if cpu.has(cpu.Feature.Bmi2) { ... }`
```

Built with `--cpu x86-64-v3`, whose level has BMI2, the test is answered
when compiling and only the instruction's way is in the program.

## Many numbers at once: vectors

The arithmetic of float arrays, their `sum` and `math.dot` are computed with
the processor's vector instructions, with nothing to write. A loop of
another shape is written with
[`std.simd`](../language/10-numbers.md#vectors). This one counts the lines
of a text, a byte at a time, sixteen at a time and thirty-two at a time:

```tondo
import std.console
import std.simd

/// How many lines a text has: its line feeds, one byte at a time.
pub fn count(bytes: Array[Byte]): Int {
    var lines = 0
    for byte in bytes {
        if byte == 10 {
            lines += 1
        }
    }
    lines
}

/// The same, sixteen bytes at a time.
pub fn countByVectors(bytes: Array[Byte]): Int {
    let feed = simd.Vector128[Byte].all(10)
    var lines = 0
    var at = 0
    for at + 16 <= bytes.length() {
        lines += simd.Vector128[Byte].load(ref bytes, at: at).equal(feed).count()
        at += 16
    }
    lines + count(bytes[at:])
}

/// The same, thirty-two bytes at a time.
pub fn countByWideVectors(bytes: Array[Byte]): Int {
    let feed = simd.Vector256[Byte].all(10)
    var lines = 0
    var at = 0
    for at + 32 <= bytes.length() {
        lines += simd.Vector256[Byte].load(ref bytes, at: at).equal(feed).count()
        at += 32
    }
    lines + count(bytes[at:])
}

fn main() {
    let text = "one\ntwo\nthree\nfour\nfive\nsix\nseven\n".bytes()
    console.println("{count(text)} {countByVectors(text)} {countByWideVectors(text)}")
}
```

```text output
7 7 7
```

Over a page of a thousand lines:

```text
$ tondo test --bench
test lines::unit::lines::aPage ... ok
  a byte at a time     17.11 µs/op  ±0.2%  0 allocs/op  0 B/op  231.1k ins/op
  by vectors           1.655 µs/op  ±0.3%  1 allocs/op  32 B/op  50.2k ins/op
  by wide vectors      912.9 ns/op  ±0.1%  1 allocs/op  48 B/op  25.9k ins/op
```

The vectors of 128 bits are accelerated on both architectures. Those of 256
bits are on an x64 processor with AVX2, and a build need not be for it: a
release build compiles a function that uses them a second time, for
x86-64-v3, and the program picks the version its processor runs when it
starts. `tondo asm countByWideVectors` shows both, the second with AVX2's
comparison of thirty-two bytes:

```text
countByWideVectors (x86-64-v3) (1088 bytes, 8 of them constants)
    ...
    26b  vmovdqu ymm1, ymmword [rdi + rcx * 1 + 0x20]
    271  vpcmpeqb ymm1, ymm1, ymm0
    275  vpmovmskb edx, ymm1
```

Where the processor lacks the instructions of a width, as an arm64 one
lacks those of 256 bits, a vector is computed by its halves: the results are
the same, and the wider type is then slower than the narrower. Vectors of
512 bits are computed so everywhere today. `accelerated()` says which a type
is where the code runs, and `TONDO_VERSIONS=0` in a program's environment
keeps it to the level it was built for.

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
