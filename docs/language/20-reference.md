# A quick reference

The language on a few pages, for looking something up. Each form links to
the chapter that explains it, and every block here compiles.

- [Declarations](#declarations)
- [Types](#types)
- [Functions](#functions)
- [Control flow](#control-flow)
- [Patterns](#patterns)
- [Errors](#errors)
- [Collections and text](#collections-and-text)
- [Changing and lending](#changing-and-lending)
- [Tasks](#tasks)
- [Tests](#tests)
- [If you come from another language](#if-you-come-from-another-language)

## Declarations

```tondo
import std.console
import std.time as clock

const MaxRetries: Int = 3

pub type User = { name: String, age: Int = 0, priv token: String = "" }

pub type UserId = Int

alias Scores = Map[String, Int]

pub enum Status {
    Active
    Suspended(String)
    Deleted { at: String }
}

context Request = { id: String = "" }

annotation Check = { min: Int = 0 }

trait Named {
    fn name(self): String
}

impl Named for User {
    fn name(self): String {
        self.name
    }
}

fn main() {
    let fixed = 1
    var changing = fixed
    changing += MaxRetries
    let (left, right) = ("a", UserId(2))
    let scores: Scores = ["ada": changing]
    let status = Status.Suspended("spam")
    console.println("{left} {right.value} {scores} {status == Status.Active} {clock.seconds(1).seconds()}")
    console.println("{User { name: "ada" }.name()} {Request.id.isEmpty()}")
}
```

```text output
a 2 [ada: 4] false 1
ada true
```

[Values](02-values.md) · [Records](05-records.md) · [Enums](06-enums.md) ·
[Traits](11-generics.md) · [Contexts](14-contexts.md) ·
[Modules](15-modules.md) · [Annotations](17-meta.md)

## Types

| Type | Is |
| --- | --- |
| `Int`, `UInt`, `Int8`…`Int128`, `UInt8`…`UInt128` | integers |
| `Float`, `Float32` | floats |
| `Bool`, `Char`, `Byte`, `String` | truth, a character, a byte, text |
| `Unit`, `Never` | `()`, and what does not come back |
| `T?` | `Option[T]`: `some(x)` or `none` |
| `T ! E` | `Result[T, E]`: `ok(x)` or `err(e)` |
| `!E` | a result with no value |
| `A \| B` | a value of one of the types |
| `(A, B)` | a tuple |
| `Array[T]`, `Map[K, V]`, `Set[T]`, `Deque[T]` | the collections |
| `Range[T]` | `a..b`, `a..=b` |
| `fn(A, B): C ! E` | a function |
| `Ref[T]`, `WeakRef[T]` | a value with an identity; a look at one |
| `Join[T]`, `Group[T]` | a running task; several |
| `Sender[T]`, `Receiver[T]` | the ends of a channel |
| `Pointer[T]` | an address, used inside `unsafe` |

As bounds only: `Display`, `Ordered`, `Iterator[T]`, `Additive`, `Number`,
`Integer`, `Copy`, `Equatable`, `Key`, `Send`, `Share`, `Call[fn(A): B]`,
`CallMut[...]`, `CallOnce[...]`.

## Functions

```tondo
import std.console

fn action() {}

fn add(a: Int, b: Int = 1): Int {
    a + b
}

fn save(path: String): !String {
    if path.isEmpty() {
        fail "no path"
    }
}

fn parse(text: String): Int ! String {
    Int.parse(text).mapErr((_) { "not a number" })
}

fn total(values: ...Int): Int {
    values.sum()
}

fn first[T](values: Array[T]): T? {
    values.get(0)
}

fn show[T: Display](value: T): String {
    "<{value}>"
}

fn User.greet(self): String {
    "hello {self.name}"
}

fn User.rename(mut self, name: String) {
    self.name = name
}

fn counter(): impl CallMut[fn(): Int] {
    var count = 0
    (): Int {
        count += 1
        count
    }
}

type User = { name: String }

fn main() {
    action()
    var user = User { name: "ada" }
    user.rename("Ada")
    let double = (n: Int): Int { n * 2 }
    var next = counter()
    console.println("{add(1)} {add(1, b: 5)} {save("x").isOk()} {parse("7")} {total(1, 2, 3)} {first([9])}")
    console.println("{show(1.5)} {user.greet()} {double(4)} {[1, 2].map((n) { n + 1 })} {next()} {next()}")
}
```

```text output
2 6 true ok(7) 6 some(9)
<1.5> hello Ada 8 [2, 3] 1 2
```

[Functions and closures](03-functions.md) · [Generics](11-generics.md)

## Control flow

```tondo
import std.console

fn main() {
    let n = 7
    let size = if n > 5 { "big" } else { "small" }

    for i in 0..3 {
        if i == 1 {
            continue
        }
        console.println("{i}")
    }

    var countdown = 2
    for countdown > 0 {
        countdown -= 1
    }

    for {
        break
    }

    let kind = match n % 2 {
        0 => "even"
        _ => "odd"
    }

    defer console.println("leaving")
    console.println("{size} {kind} {countdown} {n in 1..10} {not (n > 3 and n < 5)}")
}
```

```text output
0
2
big odd 0 true true
leaving
```

[Control flow and operators](04-control-flow.md)

## Patterns

```tondo
import std.console

type Point = { x: Int, y: Int }

enum Shape {
    Dot
    Circle(Int)
    Box { width: Int, height: Int }
}

fn describe(value: Shape | Int | Point | String): String {
    match value {
        Shape(shape) => match shape {
            Shape.Dot => "a dot"
            Shape.Circle(radius) if radius > 10 => "a big circle"
            Shape.Circle(radius) => "a circle of {radius}"
            Shape.Box { width, .. } => "a box {width} wide"
        }
        Int(0) => "zero"
        Int(number) => "the number {number}"
        Point(Point { x: 0, y }) => "on the axis at {y}"
        Point(point) => "a point at {point.x}"
        _ => "something else"
    }
}

fn main() {
    console.println("{describe(Shape.Circle(3))} | {describe(0)} | {describe(Point { x: 0, y: 4 })}")
    let list = match [1, 2, 3] {
        [] => "empty"
        [only] => "one: {only}"
        [first, ..rest] => "{first} and {rest.length()} more"
    }
    let pair = match (1, "one") {
        (1, name) => name
        _ => "other"
    }
    let maybe = match some(3) {
        some(value) => value
        none => 0
    }
    console.println("{list} | {pair} | {maybe}")
}
```

```text output
a circle of 3 | zero | on the axis at 4
1 and 2 more | one | 3
```

[Enums, unions and match](06-enums.md)

## Errors

```tondo
import std.console
import std.fs

fn lines(path: String): Int ! fs.FsError {
    let text = fs.readText(path)?           // pass the failure on
    text.lines().length()
}

fn describe(path: String): String ! Failure {
    let count = lines(path).context("counting {path}")?     // say what was going on
    "{count} lines"
}

fn main() {
    match lines("/missing") {               // handle it
        ok(count) => console.println("{count}")
        err(error) => console.println("failed: {error}")
    }
    console.println("{lines("/missing") ?? 0} {describe("/missing")}")
    let caught = recover { [1, 2][5] }      // stop a panic
    let missing: Int? = none
    let found: Int? = missing.map((n) { n + 1 })  // a chain of an option
    console.println("{caught.isErr()} {found} {some(2).map((n) { n * 2 })}")
    assert(1 + 1 == 2, "arithmetic holds")
}
```

```text output
failed: not found
0 err(counting /missing: not found)
true none some(4)
```

[Errors](07-errors.md)

## Collections and text

```tondo
import std.console

fn main() {
    var list = [3, 1, 2]
    list.push(4)
    list.sort()
    var ages = ["ada": 36]
    ages["bo"] = 20
    var seen = Set[1, 2]
    _ = seen.insert(3)

    console.println("{list} {list[0]} {list[-1]} {list[1:3]} {list.get(9)} {list * 2}")
    console.println("{ages["ada"]} {ages["cy"] ?? 0} {"bo" in ages} {seen} {(1..=3).collect()}")
    console.println("{list.map((n) { n * n }).filter((n) { n > 4 }).sum()} {list.sortedBy((n) { -n })}")

    let name = "Ada"
    let price = 3.14159
    console.println("{name:>6}|{price:.2}|{255:#x}|{name.toUpper()}|{"a,b".split(",")}|{name[0]}")
    console.println(r"raw {text} \n")
}
```

```text output
[1, 2, 3, 4] 1 4 [2, 3] none [2, 4, 6, 8]
some(36) 0 true Set[1, 2, 3] [1, 2, 3]
25 [4, 3, 2, 1]
   Ada|3.14|0xff|ADA|[a, b]|A
raw {text} \n
```

[Collections](08-collections.md) · [Text](09-text.md) · [Numbers](10-numbers.md)

## Changing and lending

```tondo
import std.console

fn read(values: ref Array[Int]): Int {
    values.sum()
}

fn change(values: mut Array[Int]) {
    values[0] = 100
}

fn grow(values: var Array[Int]) {
    values.push(0)
}

type Node = { name: String }

fn main() {
    let original = [1, 2, 3]
    var copy = original                 // a copy: `original` stays
    change(mut copy)
    grow(var copy)
    change(mut copy[1:3])               // part of it
    for mut value in copy {
        value += 1
    }
    console.println("{original} {copy} {read(ref copy)}")

    let shared = Ref(Node { name: "one" })
    let same = shared                   // the same object
    console.println("{shared == same} {shared == Ref(Node { name: "one" })} {same.value.name}")
}
```

```text output
[1, 2, 3] [101, 101, 4, 1] 207
true false one
```

[Ownership](12-ownership.md)

## Tasks

```tondo
import std.console
import std.sync
import std.time

fn work(n: Int): Int {
    time.sleep(time.milliseconds(1))
    n * 2
}

fn produce(out: Sender[Int]) {
    out.send(1)
    out.send(2)
}

fn main() {
    let job = spawn work(1)
    var group = Group[Int]()
    group.add(spawn work(2))
    group.add(spawn work(3))
    console.println("{job.wait()} {group.wait()}")

    let (sender, receiver) = channel[Int](4)
    spawn produce(sender)
    var received: Array[Int] = []
    for value in receiver {
        received.push(value)
    }

    let counter = sync.Atomic(0)
    let state = sync.Mutex([0])
    _ = counter.add(1)
    state.update((values) { values.push(1) })

    let hurried = within time.milliseconds(50) { work(4) }
    let first = select {
        let value = receiver.receive() => "received {value}"
        time.sleep(time.milliseconds(1)) => "nothing"
    }
    console.println("{received} {counter.load()} {state.get()} {hurried} {first}")
    spawn detached work(5)
}
```

```text output
2 [4, 6]
[1, 2] 1 [0, 1] ok(8) received none
```

[Tasks](13-tasks.md) · [Contexts and deadlines](14-contexts.md)

## Tests

```tondo test
import std.testing

fn double(n: Int): Int {
    n * 2
}

test doubles {
    assert(double(2) == 4)
}

suite edges {
    let big = Int.max

    test overflowIsCaught {
        assert(recover { double(big) }.isErr())
    }

    test isMeasured {
        testing.bench("double", () { double(21) })?
    }

    test holdsForAnyInput {
        testing.fuzz("halves", [0, 7], (n: Int32) {
            assert(double(Int(n)) / 2 == Int(n))
        })
    }
}
```

[Testing](16-testing.md)

## If you come from another language

| You would write | In Tondo |
| --- | --- |
| `let mut x = 0`, `x := 0` | `var x = 0` |
| `while c { }`, `loop { }` | `for c { }`, `for { }` |
| `fn f() -> Int`, `func f() int` | `fn f(): Int` |
| `fn id<T>(x: T)`, `Vec<T>`, `[]T`, `List<T>` | `fn id[T](x: T)`, `Array[T]` |
| `HashMap<K, V>`, `map[K]V`, `Dictionary<K, V>` | `Map[K, V]` |
| `null`, `nil`, `None`, `Some(x)` | `none`, `some(x)` |
| `x as Int`, `int(x)`, `(int) x` | `Int(x)`, which may give a result |
| `\|x\| x * 2`, `x => x * 2`, `func(x int) int { ... }` | `(x) { x * 2 }` |
| `struct P { }` and `impl P { fn m() }` | `type P = { }` and `fn P.m(self)` |
| `class`, `interface` | `type`, `trait` with `impl` |
| `try`, `catch`, `throw` | `T ! E`, `?`, `match`, `fail error` |
| `if err != nil { return err }` | `?` |
| `async`, `await` | plain calls; `spawn f()` and `.wait()` |
| `go f()`, a task nobody awaits | `spawn detached f()` |
| `switch`, `when` | `match` |
| `&&`, `\|\|`, `!x` | `and`, `or`, `not x` |
| `len(x)`, `x.len()`, `x.Length`, `x.size()` | `x.length()` |
| `` `${x}` ``, `f"{x}"`, `$"{x}"`, `"{}".format(x)` | `"{x}"` |
| `a + b` on strings | `"{a}{b}"`, or `a.concat(b)` |
| `x ?? y`, `x ?: y` | `x.unwrapOr(y)` |
| `a?.b` | `try { a?.b }`, or `a.map((v) { v.b })` |
| `cond ? a : b` | `if cond { a } else { b }` |
| `x++` | `x += 1` |

`tondo guide pitfalls` has the same with the error each mistake gives, and
[the comparison](../comparison.md) goes language by language.
