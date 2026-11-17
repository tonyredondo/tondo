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
- [What an executable weighs: size](#what-an-executable-weighs-size)
- [Where a build's time goes: timings](#where-a-builds-time-goes-timings)
- [Against another version](#against-another-version)
- [The processor a build is for](#the-processor-a-build-is-for)
- [What the places for logpoints cost](#what-the-places-for-logpoints-cost)
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

A run made while the machine had more to do than it has processors says so
in its last line, `note: the machine was busy (load 32.9 on 32 processors):
times are not to be compared, instructions are`.

When what is slow is one call with the values it had, a dump of that
moment gives the benchmark. This program is stuck testing a large prime
the slow way; it was built with `--debug`, so that the dump has its
values, and started with `TONDO_DIAGNOSTICS=1`:

```text
$ tondo dump primes --output slow.dump.json
$ tondo dump read slow.dump.json locals
src/main.to:4 in isPrime
number: Int = 2147483647
divisor: Int = 62746386
$ tondo dump test slow.dump.json --bench
src/main_test.to: `benchIsPrime` measures `isPrime` with the dump's values: `tondo test --bench --filter benchIsPrime`
```

It wrote, in the project's tests:

```text
// From a dump of the program as it ran
test benchIsPrime {
    let number = 2147483647
    testing.bench("isPrime", () { isPrime(number) })
}
```

That benchmark is the slow call, to measure before and after a change,
without the program around it.

A [recording](debugging.md#backwards-a-recorded-run) gives it too, of a
run that has ended. `--function` names whose call to take: the one that
ran the most lines, or with `--at <probe>` the one that ran there. This
program called `squares` with 10, 5000 and 40:

```text
$ tondo run --record run.rec
$ tondo dump test run.rec --bench --function squares
src/main_test.to: `benchSquares` measures `squares` with the recording's values: `tondo test --bench --filter benchSquares`
```

```text
// From a recording: the call of squares at probe 44, which ran 15003 lines
test benchSquares {
    let upTo = 5000
    testing.bench("squares", () { squares(upTo) })
}
```

## Where the time goes: profiles

`tondo profile` samples a running program, or runs one from its start, and
`tondo test --bench --profile cpu` profiles a benchmark
([A running program](observing.md#where-the-time-goes-profile)).

```text
tondo profile -- ./report big.csv         a program, from its start
tondo profile api --cpu --duration 10     one that is running
tondo test --exact <id> --bench --profile cpu
tondo test --exact <id> --bench --profile instructions
```

`tondo profile show` reads the result three ways, each closer to the
metal:

| | |
| --- | --- |
| `tondo profile show cpu.pb.gz` | the functions that take the most |
| `tondo profile show cpu.pb.gz --lines` | the lines |
| `tondo profile show cpu.pb.gz --asm` | the instructions of the hottest function, each with its share of the samples |
| `tondo profile show cpu.pb.gz --html flame.html` | a flame graph, as one file |

By line, a profile says where the time goes and, under each line of the
program, what the compiler made of it, in the words of `tondo
optimizations` (below). This is a benchmark of a function that counts the
letters of a text:

```text
$ tondo test --bench --profile cpu
$ tondo profile show target/profiles/letters.unit.letters.counts--letters.cpu.pprof --lines
cpu of target/profiles/letters.unit.letters.counts--letters.cpu.pprof, 968ms in all over 1.05s
     flat  flat%       cum   cum%  line
    653ms  67.5%     653ms  67.5%  src/main.to:5 in letters  if c.isLetter() {
                                     `c.isLetter()` is inlined: the body of `Char.isLetter` is copied here (in closure in test counts, letters)
    219ms  22.6%     219ms  22.6%  src/main.to:4 in letters  for c in text.chars() {
                                     `text.chars()` in `prelude.CharIterator.next`, `self.offset += if code < 0x800 { 2 } else if code…` keeps its overflow check, in a loop: its operands' bounds allow a result past `Int.Max` (in closure in test counts, letters)
                                     `text.chars()` is inlined: the body of `String.chars` is copied here (in closure in test counts, letters)
                                     `text.chars()` is inlined: the body of `prelude.CharIterator.next` is copied here (in closure in test counts, letters)
```

So the question that follows a profile, why that line costs what it does,
is answered where the line is: two thirds of the time is the test of each
character, and the loop keeps a check for each. A line shows six remarks at
most and says how many more there are. They are read from the sources at
hand when those are the ones the program was built from; when they are
not, as after an edit, or for a build made with `--debug`, the view says
why it shows none.

A profile by time samples what the clock says, and a machine that is busy
with other work stretches every line of it. On Linux, `--instructions`
samples by the instructions each thread ran instead, one sample each
million, so the same program gives the same profile whatever else the
machine does. It reads the same way, in instructions:

```text
$ tondo test --bench --profile instructions
$ tondo profile show target/profiles/letters.unit.letters.counts--letters.instructions.pprof --lines
instructions of target/profiles/letters.unit.letters.counts--letters.instructions.pprof, 6.66G in all over 0.57s
     flat  flat%       cum   cum%  line
    4.56G  68.5%     4.56G  68.5%  src/main.to:5 in letters  if c.isLetter() {
                                     `c.isLetter()` is inlined: the body of `Char.isLetter` is copied here (in closure in test counts, letters)
    1.42G  21.2%     1.42G  21.2%  src/main.to:4 in letters  for c in text.chars() {
```

Where the system does not let a program count its instructions, as a
container does by default, the command says so before it builds and names
`--cpu`.

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
stats.to:19:5: `"{sensor}: {value:.1}"` is not made in the frame: it is copied (in main)
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is returned (in label)
stats.to:19:5: `"{sensor}: {value:.1}"` goes in a region of `main` (in main)
stats.to:19:5: `"{sensor}: {value:.1}"` stays on the heap: `label` returns it with what it was given (in label)
stats.to:19:17: `value` in `formats.formatFloat`, `formatNumber(` stays a call of `formats.formatNumber`: its body costs 31, more than the 30 inlining takes (in label)
stats.to:19:17: `value` stays a call of `formats.formatFloat`: its body costs 80, more than the 30 inlining takes (in main)
stats.to:19:17: `value` is inlined: the body of `formats.formatFloat` is copied here (in label)
stats.to:24:14: `0..1000` runs four turns at a time (in main)
stats.to:25:9: `readings.push(Reading { sensor: if n % 2 == 0 { "a" } else …` grows an array that goes in a region of `main` (in main)
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
- **The heap.** In `label`, the label is a string that is returned, so it
  is made on the heap. A string, a closure or a record that does not
  outlive its function stays in the function's frame, and the line says
  which of the two happened and what decided it.
- **Regions.** The array of readings never leaves `main`, and its size is
  not known when compiling, so it goes in a region of `main`: it is made
  by moving a pointer and goes with the region when `main` ends
  ([What a region takes](#what-a-region-takes)). So does the label where
  `main` inlined `label`: the frame does not take it, and the line says
  why, and the region does. Each place has a line for each copy of its
  code, named at its end.
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
kept.to:8:9: `reading` keeps a count of its own: what it is given to may keep it: `found.push(reading)` at 10:13 (in main, of)
kept.to:8:20: `readings` runs one turn at a time: each turn counts a value or measures a text, which costs more than the test saved (in main, of)
kept.to:10:13: `found.push(reading)` grows as it is pushed to: the loop pushes to it in some turns only (in main, of)
kept.to:10:13: `found.push(reading)` grows an array that goes in a region of `main` (in main)
kept.to:10:13: `found.push(reading)` grows an array that stays on the heap: `of` returns it, and no call of it is left in the program's code: each was inlined (in of)
```

Each remark names what the code does that the optimization could not get
past: the element is kept, by the push the remark names, so it is counted; counting makes a turn too
heavy to run four at a time; and the array cannot be given its room
beforehand, since only some turns push. None of it is wrong here, and the
view is how one knows that the cost is the program's and not an accident.
The last two lines are of where the array is made: in the copy of `of` that
was inlined in `main`, which lets go of it, it goes in a region of `main`.

| | |
| --- | --- |
| `--function <name>` | one function, and the code written in it wherever it went |
| `--kind heap` | only what allocates |
| `--kind region` | what goes in a [region](#what-a-region-takes), and what stays on the heap and why; with it, also what the library's code copied into a function makes on its way and lets go of inside the call, as the texts that write `{value:.1}` |
| `--kind hoisted`, `--kind reloaded` | the [lengths](#a-length-read-once) a loop reads once, and those it reads each turn |
| `--tests` | the tests and benchmarks instead of the program |
| `--against <rev>` | only what a change made different |
| `--json` | the same as data |

```text
$ tondo optimizations stats.to --kind heap
stats.to:19:5: `"{sensor}: {value:.1}"` is made on the heap: it is returned (in label)
```

Only the `label` that stays a function allocates on the heap, and its one
call was inlined: the copy in `main` has the label in a region.

`tondo check --performance` warns, while writing, of costs that the
compiler cannot take away, because doing so would change what the program
means: a copy that a loop makes on every turn (`W1009`), an array that a
loop searches while it pushes to it (`W1023`), taking the first element
of an array in a loop (`W1024`), and compiling a regular expression each
time the code runs (`W1025`). This program has the last two:

```tondo
import std.console
import std.regex

/// The lines of a text after the blank ones at its start.
fn body(lines: Array[String]): Array[String] {
    var rest = lines
    for not rest.isEmpty() and rest[0].trim().isEmpty() {
        _ = rest.removeAt(0)
    }
    rest
}

/// Whether a text is one word.
fn isWord(text: String): Bool {
    match regex.Regex.compile(r"^\w+$") {
        ok(pattern) => pattern.isMatch(text) ?? false
        err(_) => false
    }
}

fn main() {
    console.println("{body(["", " ", "a", "b"])} {isWord("tea")}")
}
```

```text output
[a, b] true
```

```text
$ tondo check queue.to --performance
warning[W1024]: `rest.removeAt(0)` moves every other element of `rest` on every turn: a `Deque` takes its first at once, with `popFront()`; or find where to cut once and take a `slice`
warning[W1025]: `regex.Regex.compile(r"^\w+$")` compiles its pattern every time this code runs: a context's field compiles it once, when first read: `context Patterns = { pattern: regex.Regex = regex.Regex.compile(r"^\w+$")? }`, and `Patterns.pattern` reads it
```

Each says what to write. The loop that keeps what it has not seen yet is
the one `W1023` is for: it looks through an array that grows with every
turn.

```tondo
import std.console

/// The names, each once, in the order they came.
fn once(names: Array[String]): Array[String] {
    var seen: Array[String] = []
    for name in names {
        if not seen.contains(name) {
            seen.push(name)
        }
    }
    seen
}

fn main() {
    console.println("{once(["ada", "bo", "ada"])}")
}
```

```text output
[ada, bo]
```

```text
$ tondo check names.to --performance
warning[W1023]: `seen.contains(name)` looks at every element of `seen`, which this loop pushes to, so each turn looks at more: keep a `Set` beside it, `var seenSet = seen.toSet()` before the loop, ask `seenSet.contains(...)`, and `seenSet.insert(...)` with each push
```

A loop that searches an array which stays as it is gets no warning: it
is most often an array of a few elements, where a `Set` gains nothing.

What the compiler can do by itself it does, and no warning asks for it: a
text that grows in a loop, as `text = text.concat(part)`, grows in place.

### Why a check stays

A check that the compiler keeps says what it would need to know to drop
it, and the code of the standard library that a loop runs is said at the
program's line that brought it in. This function walks a text:

```tondo
import std.console

/// How many letters a text has.
fn letters(text: String): Int {
    var count = 0
    for c in text.chars() {
        if c.isLetter() {
            count += 1
        }
    }
    count
}

fn main() {
    console.println("{letters("hello, world")}")
}
```

```text output
10
```

```text
$ tondo optimizations letters.to --kind checked
letters.to:6:14: `text.chars()` in `prelude.CharIterator.next`, `self.offset += if code < 0x800 { 2 } else if code…` keeps its overflow check, in a loop: its operands' bounds allow a result past `Int.Max` (in letters, main)
letters.to:8:13: `count += 1` keeps its overflow check, in a loop: nothing bounds `count` from above (in letters, main)
```

The second line is the program's own: nothing says how large `count` can
get, so each turn checks that adding one fits. The first is of the standard
library, in the method that gives the next character, which the loop over
`chars()` copied into `letters`: its line is the program's, and it names
where the code is from and the statement checked. A check that the
compiler could drop, as the one of `count += 1` in the first program of
this chapter, is under the kind `unchecked`.

A program that has a `recover` anywhere, as tests often do, pays one more
thing in its loops: after what may panic, it looks whether it did. The
same function, called as `recover { letters("hello, world") }`, shows it:

```text
$ tondo optimizations caught.to --kind checked
caught.to:6:14: `text.chars()` in `prelude.CharIterator.next`, `self.offset += if code < 0x800 { 2 } else if code…` is followed by a check for a panic, in a loop: a program with a `recover` checks after what may panic (in letters, main)
caught.to:6:14: `text.chars()` in `prelude.CharIterator.next`, `self.offset += if code < 0x800 { 2 } else if code…` keeps its overflow check, in a loop: its operands' bounds allow a result past `Int.Max` (in letters, main)
caught.to:7:12: `c.isLetter()` in `Char.isLetter`, `textNative.charIs(self, 1)` is followed by a check for a panic, in a loop: a program with a `recover` checks after what may panic (in letters, main)
caught.to:8:13: `count += 1` is followed by a check for a panic, in a loop: a program with a `recover` checks after what may panic (in letters, main)
caught.to:8:13: `count += 1` keeps its overflow check, in a loop: nothing bounds `count` from above (in letters, main)
```

So a benchmark is not always the cost of the program: the build of the
tests may check what the program's build does not, and `tondo
optimizations --tests` says where.

### A length read once

A loop that compares its place with the length of an array or of a text
reads that length once, before the loop, when nothing in the loop can
change it; `--kind hoisted` lists those, and `--kind reloaded` the ones it
reads at every turn, with why.

```tondo
import std.console

/// How many of the values are even: the loop only reads `values`.
fn evens(values: Array[Int]): Int {
    var count = 0
    var i = 0
    for i < values.length() {
        if values[i] % 2 == 0 {
            count += 1
        }
        i += 1
    }
    count
}

/// Each even value followed by the number after it: the loop pushes to
/// what it reads.
fn spread(values: Array[Int]): Array[Int] {
    var out = values
    var i = 0
    for i < out.length() {
        if out[i] % 2 == 0 and out.length() < 8 {
            out.push(out[i] + 1)
        }
        i += 1
    }
    out
}

fn main() {
    console.println("{evens([1, 2, 3, 4])} {spread([2, 5])}")
}
```

```text output
2 [2, 5, 3]
```

```text
$ tondo optimizations lengths.to --kind hoisted
lengths.to:7:13: `values.length()` reads the length once, before the loop (in evens, main)
$ tondo optimizations lengths.to --kind reloaded --function spread
lengths.to:21:13: `out.length()` reads the length each turn: the loop may change `out` (in spread)
lengths.to:21:13: `out.length()` reads the length each turn: the loop may change what it is the length of (in main)
lengths.to:22:32: `out.length()` reads the length each turn: the loop may change `out` (in spread)
lengths.to:22:32: `out.length()` reads the length each turn: the loop may change what it is the length of (in main)
```

`spread` pushes to the array whose length it tests, so the length is read
each turn, as the program says. A length is also read each turn when it is
reached through a loan or a lock, where other code may change it, which is
what a loop that waits for another task to push relies on.

### The closures of a chain

The adapters of an [iterator](../language/08-collections.md#iterators) keep
the closures they are given, and the method that ends the chain calls them
for every element. This program adds the squares of the even numbers twice:
with the array's own methods, and with a chain that starts at `iter()`.

```tondo
import std.console

/// The sum of the squares of the even numbers, with an array at each step.
fn byArrays(numbers: Array[Int]): Int {
    numbers.filter((n) { n % 2 == 0 }).map((n) { n * n }).sum()
}

/// The same, a number at a time.
fn byChain(numbers: Array[Int]): Int {
    numbers.iter().filter((n) { n % 2 == 0 }).map((n) { n * n }).sum()
}

fn main() {
    let numbers = (1..=1000).collect()
    console.println("{byArrays(numbers)} {byChain(numbers)}")
}
```

```text output
167167000 167167000
```

```text
$ tondo optimizations squares.to --function byChain --kind call
squares.to:10:5: `numbers.iter().filter((n) { n % 2 == 0 }).map((n) { n * n }…` in `Iterator.sum[prelude.MapIterator[prelude.FilterIterator[prelude.ArrayIterator[Int], Int], Int, Int], Int]`, `self.sumBy((item) { item })` stays a call of `Iterator.sumBy[prelude.MapIterator[prelude.FilterIterator[prelude.ArrayIterator[Int], Int], Int, Int], Int, Int]`: its body costs 48, more than the 30 inlining takes (in byChain, main)
squares.to:10:27: `(n) { n % 2 == 0 }` is called by its body where a record holds it, as an iterator's adapter does (in prelude.FilterIterator.next[prelude.ArrayIterator[Int], Int])
squares.to:10:51: `(n) { n * n }` is called by its body where a record holds it, as an iterator's adapter does (in Iterator.sumBy[prelude.MapIterator[prelude.FilterIterator[prelude.ArrayIterator[Int], Int], Int, Int], Int, Int])
```

The chain is run by the standard library's `sumBy`, which `sum` calls: a
function of its own, not copied into `byChain`, as the first line says. A release build makes a copy
of it for this chain in which it knows the closure that each adapter holds:
asking the filter and the map for the next number calls the body of each
closure, and no longer a value that could be any function. The lines name
the functions of the standard library where those calls are, and
`--function byChain` gives them because the closures are written in
`byChain`.

The two closures are still allocated, each time `byChain` runs:

```text
$ tondo optimizations squares.to --function byChain --kind heap
squares.to:10:27: `(n) { n % 2 == 0 }` is made on the heap: a value holds it: `numbers.iter().filter((n) { n % 2 == 0 }).map((n)…` at 10:5 (in byChain, main)
squares.to:10:51: `(n) { n * n }` is made on the heap: a value holds it: `numbers.iter().filter((n) { n % 2 == 0 }).map((n)…` at 10:5 (in byChain, main)
```

Those of `byArrays` are made in the frame, since its methods only call
them: `--kind frame` lists them. Each step of `byArrays` makes an array,
though. A benchmark of the two over a thousand numbers shows both costs:

```text
$ tondo test --bench
test squares::unit::squares::aThousand ... ok
  arrays     1.104 µs/op  ±0.1%  0 allocs/op +9 in regions  0 B/op +12448 in regions  23.6k ins/op
  chain      1.397 µs/op  ±0.1%  2 allocs/op  80 B/op  38.2k ins/op
```

The chain allocates its two closures and nothing else, however long the
array is: less than a hundredth of the memory here. The arrays' form makes
its arrays in a region, which the row counts apart from the heap, and is
still the faster of the two, with two thirds of the instructions.

### What a region takes

Code that builds a large structure and lets go of all of it at once, as a
parser's tree or a document read to answer one question, spends much of its
time letting go, value by value. A release build puts in a region what it
proves never leaves a scope: a value there is made by moving a pointer, and
the region goes whole when the scope ends, with no value visited. Nothing
is written for it, and a build with `--debug` or a live one has none.
`tondo optimizations` says, of each place that allocates, the region its
values go in, or what keeps them on the heap.

```tondo
import std.console

type Node = {
    value: Int
    children: Array[Node]
}

/// A tree of that depth, each node with `fan` children.
fn build(depth: Int, fan: Int): Node {
    var children: Array[Node] = []
    if depth > 0 {
        for _ in 0..fan {
            children.push(build(depth - 1, fan))
        }
    }
    Node { value: depth, children }
}

fn sum(node: Node): Int {
    var total = node.value
    for child in node.children {
        total += sum(child)
    }
    total
}

fn main() {
    console.println("{sum(build(8, 4))}")
}
```

```text output
29124
```

A benchmark of `sum(build(8, 4))`, as a release build makes it and then
without regions, which `--no-regions` leaves out of a build:

```text
$ tondo test --bench
test tree::unit::tree::aTree ... ok
  tree of 87 thousand nodes     758.5 µs/op  ±0.2%  0 allocs/op +21845 in regions  0 B/op +2097120 in regions  15.37M ins/op
$ tondo test --bench --no-regions
test tree::unit::tree::aTree ... ok
  tree of 87 thousand nodes     1.085 ms/op  ±0.6%  21845 allocs/op  2097120 B/op  21.10M ins/op
```

The tree's arrays are still made, all 21845 of them: the row counts them
`in regions`, apart from `allocs/op`, which is the heap's. What is gone is
the walk that let go of each, and with it 30% of the time.
`testing.allocations` counts the two together, so a test that holds a
function to its count does not change when its values move to a region
([Testing](testing.md)).

```text
$ tondo optimizations tree.to --kind region
tree.to:13:13: `children.push(build(depth - 1, fan))` grows an array that goes in a region of `main` (in main)
tree.to:13:13: `children.push(build(depth - 1, fan))` grows an array that goes in a region that the caller of `build` gives (in build)
```

There are three regions a value can go in:

- **Of its function**, when nothing keeps it beyond the call, as the array
  of readings of the first program of this chapter.
- **Of a turn of a loop**, when it does not outlive the turn: the region is
  emptied at each turn, as the benchmark's own loop does with each tree.
- **One its caller gives**, when the function only returns what it makes,
  new, and the caller lets go of it: `build` here. The function builds in
  its caller's region, in a copy of it made for the calls that let its
  result go.

The first line is of the copy of `build` that was inlined in `main`: the
code of several levels of `build` is one loop there, and what it makes goes
in the region of `main`, which lets go of the tree when it ends. The second
is of `build` where it stays a call, as in the benchmark.

A place stays on the heap when something keeps what it makes beyond the
scope, and the remark names the code that does. The verdict is by call.
With a function added to the program that sends a tree through a channel,
`fn offer(sender: Sender[Node], depth: Int) { sender.send(build(depth, 2))
}`, which `main` calls after its sum:

```text
$ tondo optimizations offered.to --kind region --function build
offered.to:13:13: `children.push(build(depth - 1, fan))` grows an array that goes in a region of `main` (in main)
offered.to:13:13: `children.push(build(depth - 1, fan))` grows an array that goes in a region that the caller of `build` gives, in a copy of it for the calls that let it go: `main` at 33; `main` keeps it: it is sent: `sender.send(build(depth, 2))` at 29:5 (in build)
offered.to:13:13: `children.push(build(depth - 1, fan))` grows an array that stays on the heap: it is sent: `sender.send(build(depth, 2))` at 29:5 (in main, offer)
```

The call that lets its tree go still gets a region, and the tree that is
sent stays on the heap: one call that keeps does not take the region from
the others. The reasons a value stays on the heap are that it is stored in
what the function was given, in a lock's state or in a `Ref`; that a
closure captures it, that it is sent, taken by a task or given to C; that
it outlives a turn of its loop; that it holds a value its region did not
make; and that regions do not take that kind of value yet, as maps and
slices.

A text goes in a region as an array does, whatever its length, when the
frame does not take it first: one that an interpolation makes, as the
label of the first program of this chapter where `main` inlined it, and one
that an operation of the runtime or of the library gives, as `split`,
`replace` or `fs.readText`. So does an array that such an operation gives,
as `sorted` or `repeat`. This function reads a file, and none of its texts
is on the heap:

```tondo
import std.console
import std.fs

/// How many lines of a file hold the word.
fn holding(file: String, word: String): Int ! fs.FsError {
    let text = fs.readText(file)?
    var count = 0
    for line in text.lines() {
        if line.toLower().contains(word) {
            count += 1
        }
    }
    count
}

fn main() {
    match holding("notes.txt", "region") {
        ok(count) => console.println("{count}")
        err(_) => console.println("no notes")
    }
}
```

```text
$ tondo optimizations holding.to --kind region
holding.to:6:16: `fs.readText(file)` goes in a region of `holding` (in holding)
holding.to:8:17: `text.lines()` in `String.lines`, `textNative.lines(self)` goes in a region of `holding` (in holding)
holding.to:9:12: `line.toLower()` in `String.toLower`, `textNative.lower(self)` goes in the region of a turn of the loop at line 8 (in holding)
```

The file's text and its lines are there for the call, and each line in
lower case for its turn. The `?` gives the error back when the file cannot
be read, and that error holds nothing of a text, so no text leaves with it.
An error made with a text in it, as `fail Problem { line }`, takes that
text where the function's result goes, as a text that is returned. One
reason is a text's own: a text that grows in place, as `line =
"{line}[{word}]"` in a loop, stays on the heap, with `a region does not
grow a text in place yet`.

A turn's region is emptied when the turn goes back to the head of its
loop, and let go of when the function returns. So a value does not outlive
its turn by a way out of the loop that never comes back to it: a `?`, a
`return`, or the `break` of a loop that no loop around it runs again. This
function reads rows of numbers, and any of them may be no number:

```tondo
import std.console
import std.text

/// The largest sum of a row of numbers, each row a line of them between commas.
fn largest(rows: Array[String]): Int ! text.TextError {
    var most = 0
    for row in rows {
        var numbers: Array[Int] = []
        for field in row.split(",") {
            numbers.push(Int.parse(field)?)
        }
        if numbers.sum() > most {
            most = numbers.sum()
        }
    }
    most
}

fn main() {
    match largest(["3,4", "10,2,1", "5"]) {
        ok(most) => console.println("{most}")
        err(error) => console.println("{error}")
    }
}
```

```text output
13
```

```text
$ tondo optimizations rows.to --kind region
rows.to:9:22: `row.split(",")` in `String.split`, `textNative.split(self, separator, most)` goes in the region of a turn of the loop at line 7 (in largest)
rows.to:10:13: `numbers.push(Int.parse(field)?)` grows an array that goes in the region of a turn of the loop at line 7 (in largest)
```

The `?` leaves both loops with a row's numbers still to let go of, and they
are in the region of the row's turn all the same, which is there until
`largest` returns. So are the fields of the row: the first line is of the
library's code that `split` copied into `largest`, told at the program's
call, as is what such code gives back to the program. The price is that what
the last turn made is held from such a way out to the function's return: at
most what one turn makes, and felt only in a function that goes on long
after its loop. The `break` of a loop inside another is a way out that comes
back, since the loop around it runs the inner one again: what it carries
does outlive the inner loop's turn, and goes in the region of a turn of the
loop around it when it does not outlive that one too.

What regions hold is seen where the heap is: a memory profile has
`region_objects` and `region_space` beside the heap's samples, `tondo
counters` has the bytes regions hold now, and a dump lists the memory in
regions by the place that made it ([Observing](observing.md)).

`--no-regions` on `tondo build`, `run`, `test`, `size` and `asm`, or `regions =
false` under `[build]` in `tondo.toml`, builds without them: every value
that a region would take is on the heap, as in a debug build. It is how the
second row above was measured, and what to build with when a release build
gives a result that a debug build does not
([Debugging](debugging.md#a-release-build-that-gives-another-result)).

## The machine code: asm

`tondo asm` gives the instructions a release build makes of a function,
under the lines they come from, with what `optimizations` says of each
line beside it:

```text
$ tondo asm stats.to mean
mean (1144 bytes, 8 of them constants)
      0  push rbp
      1  mov rbp, rsp
      4  sub rsp, 0x80
      ...
stats.to:9  for reading in readings {
        ; four turns at once run 48 to 68 instructions on its common path, by the way it branches: no check and at most 4 calls
        ; four turns at once run 44 to 64 instructions on its common path, by the way it branches: no check and at most 4 calls
        ; a turn of the last ones, one at a time, runs 13 to 19 instructions on its common path, by the way it branches: at most 1 check and at most 1 call
        ; `reading` is lent, with no count of its own: its array holds it
        ; `readings` runs four turns at a time
     79  mov rdi, qword [rbx + rcx * 1 + 0x20]
     7e  movsd xmm0, qword [rbx + rcx * 1 + 0x28]
     84  movdqu xmmword [rsp + 0x40], xmm0
stats.to:10  if reading.sensor == sensor {
     8a  mov rcx, qword [rip]  ; tondo_rt_str_eq
     91  mov rsi, qword [rsp + 0x20]
     96  call rcx
     98  test al, al
     9a  jz $+0x1d
     a0  movdqu xmm0, xmmword [rsp + 0x40]
     a6  movdqu xmm1, xmmword [rsp + 0x30]
stats.to:11  total += reading.value
     ac  addsd xmm1, xmm0
     b0  movdqu xmmword [rsp + 0x30], xmm1
```

What the remarks said is there to see. Under line 9 the element is read
where it lies in the array, with no call to count it. Line 10 is the one
call of the loop, the comparison of two strings, and line 11 is a single
addition. Nobody has to read assembly to use Tondo, but when a question
comes down to it, the answer is one command away and is laid out by the
source.

The first lines under line 9 say what the loop costs: at the line of a
loop that has no other inside it, the listing gives the instructions a
turn runs on its common path, and how many of them are checks and calls.
This loop runs four turns at once, so the number is of the four, in each
of the two copies the compiler made of it, one that still checks `count
+= 1` and one that does not, and then of a turn of those left at the end.
A loop that runs a turn at a time has one line, as the one of "Why a check
stays":

```text
$ tondo asm letters.to letters
letters.to:6  for c in text.chars() {
        ; a turn of the loop runs 21 to 43 instructions on its common path, by the way it branches: at most 2 checks and at most 2 calls
```

That is the number to bring down, and the one to compare before and after
a change.

| | |
| --- | --- |
| `tondo asm <function>` | one function of the project |
| `tondo asm --all` | every function |
| `--clif` | Cranelift's intermediate code as well, before and after it optimizes |
| `--cpu <level>` | the code of a [level of processor](#the-processor-a-build-is-for) |
| `--tests` | a function of the tests or benchmarks |
| `--against <rev>` | the difference with a revision's code |
| `tondo profile show <profile> --asm` | the same listing, with the share of the samples on each instruction |

## What an executable weighs: size

`tondo size` says what a release build is made of, by part and then by
function. For the program of this chapter's first section:

```text
$ tondo size stats.to
2.93 MiB in all
    4.9 KiB   0.2%  the program's code
    4.0 KiB   0.1%  the standard library's code
      913 B   0.0%  code the compiler adds
   1.78 MiB  60.7%  the runtime's code
      764 B   0.0%  the program's data
  489.1 KiB  16.3%  the runtime's data
  167.1 KiB   5.6%  tables for traces, dumps and debuggers
  470.1 KiB  15.7%  the names of symbols
   41.2 KiB   1.4%  the rest: headers, dynamic linking, alignment
the functions that weigh the most:
   37.1 KiB  tondo_runtime::debugger::commands::command  (tondo_runtime::debugger)
   34.6 KiB  tondo_runtime::debugger::stopping::render  (tondo_runtime::debugger)
   32.6 KiB  tondo_runtime::diagnostics::serving::answer  (tondo_runtime::diagnostics)
```

A small program is its runtime: the scheduler of tasks, the debugger and
the diagnostics that every executable carries, and the names that a stack
and a dump read. The program's own code is the first two lines, and they
are what grows with it. `--function <name>` keeps the functions whose
names hold it, `--top <n>` gives more of them, `--tests` weighs the build
of the tests, and `--against <rev>` gives what a change added. The options
of `build` that change what is built, as `--strip` and `--static`, weigh
that build.

## Where a build's time goes: timings

`--timings`, on `build`, `check`, `run` and `test`, says where the time of
the build went, by phase, then the files and the functions that took the
code generator longest:

```text
$ tondo build stats.to --timings
the build took 595.4 ms
  reading and checking        121.4 ms
  lowering                      0.9 ms
  passes                       37.4 ms
  code generator               17.9 ms
  link                        413.9 ms
the files whose functions took the code generator longest, on one core:
  stats.to                                   16.6 ms  3 functions
  <std>/formats.to                           11.5 ms  3 functions
  <std>/prelude.to                            3.9 ms  2 functions
the functions:
  main                                       11.2 ms  stats.to
  formats.formatFloat                         5.6 ms  <std>/formats.to
  formats.formatText                          4.5 ms  <std>/formats.to
  mean                                        4.3 ms  stats.to
```

Here the link is two thirds of it, as in any small program. In a large
one, the list says which file or function to look at when a build got
slow.

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
        ; `64 - word.leadingZeros()` keeps its overflow check, in a loop: nothing bounds what is taken away from below
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
        ; `64 - word.leadingZeros()` keeps its overflow check, in a loop: nothing bounds what is taken away from below
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

### One instruction of a processor

`std.cpu.x64` and `std.cpu.arm64` have functions that are one instruction
each, as the intrinsics of other languages are, for the files of their
architecture
([Modules](../language/15-modules.md#code-for-one-system-or-architecture)).

| | `std.cpu.x64` | `std.cpu.arm64` | Needs |
| --- | --- | --- | --- |
| CRC-32C | `crc32cByte`, `crc32cInt` | `crc32cByte`, `crc32cInt` | SSE4.2, which the baseline has; `Crc32` on arm64 |
| Bits put or gathered by a mask | `depositBits`, `extractBits` | | BMI2 |
| A round of AES | `aesEncrypt`, `aesEncryptLast`, `aesDecrypt`, `aesDecryptLast` | `aesEncrypt`, `aesDecrypt`, `aesMixColumns`, `aesInverseMixColumns` | AES |
| A product without carries | `carrylessMultiply` | `polynomialMultiply` | PCLMULQDQ; PMULL on arm64 |
| A turn of a loop that waits | `pause` | `pause` | nothing |

Those of AES and of the product take and give `simd.Vector128` values, so
they work with the vectors' own operations
([Numbers](../language/10-numbers.md#vectors)).

A function that uses one needs its feature. The compiler works that out
from the bodies, for the whole program, and nothing is written for it. A
checksum with the instruction of each architecture is three files, the
common one with the way that every processor can run:

```text
// src/checksum/checksum.to
/// The CRC-32C of the bytes, the checksum of iSCSI and ext4.
pub fn crc32c(bytes: Array[Byte]): UInt32 {
    ~update(0xFFFF_FFFF, bytes)
}

/// A bit at a time, for a processor without the instruction.
fn byBits(crc: UInt32, bytes: Array[Byte]): UInt32 {
    var value = crc
    for byte in bytes {
        value = value ^ UInt32(byte)
        for _ in 0..8 {
            value = if value & 1 == 1 { (value >> 1) ^ 0x82F6_3B78 } else { value >> 1 }
        }
    }
    value
}
```

```text
// src/checksum/checksum_x64.to
import std.cpu.x64

/// SSE4.2's `crc32`, which every x64 processor of the baseline has.
fn update(crc: UInt32, bytes: Array[Byte]): UInt32 {
    var value = crc
    for byte in bytes {
        value = x64.crc32cByte(value, byte)
    }
    value
}
```

```text
// src/checksum/checksum_arm64.to
import std.cpu
import std.cpu.arm64

fn update(crc: UInt32, bytes: Array[Byte]): UInt32 {
    if cpu.has(cpu.Feature.Crc32) { byInstruction(crc, bytes) } else { byBits(crc, bytes) }
}

/// `crc32cb`, where the processor has it.
fn byInstruction(crc: UInt32, bytes: Array[Byte]): UInt32 {
    var value = crc
    for byte in bytes {
        value = arm64.crc32cByte(value, byte)
    }
    value
}
```

On x64 the instruction is of the baseline, so nothing is tested. On arm64
it is beyond it, so the call goes under a test of the processor, with the
plain way beside it. Over a page of a thousand lines, on x64:

```text
$ tondo test --bench
test crc::unit::checksum::theInstructionAndTheBitsAgree ... ok
  by the instruction     25.37 µs/op  ±0.4%  0 allocs/op  0 B/op  294.1k ins/op
  a bit at a time        213.1 µs/op  ±0.2%  0 allocs/op  0 B/op  4.07M ins/op
```

On x64 a feature beyond the baseline takes the same test. This function
uses BMI2's `pdep`:

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

## What the places for logpoints cost

A Release build keeps, at each line, a place where `tondo logpoint` and a
breakpoint of a Release program can be put while it runs
([Observing](observing.md#what-a-line-holds-logpoint)). The place is five
bytes that do nothing, and passing them is the only cost when nobody asks
for one. In most code it does not show: on the benchmarks of this repository
all but a few do under 1% more instructions with them. It shows where a line
is a handful of instructions and runs many times:

```text
$ tondo test bench/tondo --bench --filter bench::calls
  fib 25           217.8 µs/op  ±0.2%  0 allocs/op  0 B/op  4.81M ins/op
$ tondo test bench/tondo --bench --filter bench::calls --no-logpoints
  fib 25           226.9 µs/op  ±1.4%  0 allocs/op  0 B/op  3.84M ins/op
```

A quarter more instructions with the places. In time it is far less, since
an instruction that does nothing is the cheapest there is: about 5% on a
quiet machine, which is less than a busy one moves a measure by, as these
two runs show, where the build without them took longer
([What not to be fooled by](#what-not-to-be-fooled-by)). A program measured
to need those instructions back is built without the places:

| | |
| --- | --- |
| `--no-logpoints` | in `build`, `run`, `test`, `asm` and `size` |
| `logpoints = false` | in the `[build]` of the program's `tondo.toml`, for every build of it |

Such a program takes no logpoint and no breakpoint while it runs, and says
so when asked for one. Leave the places in a program that runs in
production unless a measure of that program says otherwise: they are what
lets one ask it what a line holds without building it again.

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

`simd.Vector[T]` takes the width for the code: the same function has it of
256 bits in its version for AVX2 and of 128 bits in the other, so a loop
written with it and `lanes()` uses the widest the processor does well with
nothing chosen in the source.

## Memory

| | |
| --- | --- |
| `tondo test --bench` | allocations and bytes for each operation, on the heap and in regions |
| `testing.allocations(closure)` | the two added, in a test, which fails when the count grows; `testing.heapAllocations` for the heap alone ([Testing](testing.md#benchmarks)) |
| `tondo optimizations --kind heap` | each place that allocates, and what keeps it on the heap |
| `tondo optimizations --kind region` | each place whose values go in a region, and each that stays on the heap, with why |
| `tondo profile <program> --memory` | what is allocated and what is held, by line |
| `tondo test --exact <id> --bench --profile memory` | where a benchmark allocates |
| `tondo profile show <profile> --sample alloc_objects --lines` | the lines that allocate most often |
| `tondo profile show <profile> --sample region_objects --lines` | the lines that make most values in regions |
| `tondo dump diff a.json b.json` | what grew between two moments of a running program |
| `tondo counters <program>` | memory held, what regions hold, the rate of allocation, the collector's pauses |

A memory profile samples allocations. `TONDO_HEAP_RATE=1` in the program's
environment records every one, for a benchmark where each allocation is to
be accounted for.

Memory is counted, so most of it is freed the moment its last holder goes,
and there is no collector to tune. What a server holds follows what its
data structures hold.

## What not to be fooled by

- **Time on a busy machine.** Judge by instructions, and look at time
  last. A run made under load says so in its last line, and a profile by
  instructions is the one to take then.
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
