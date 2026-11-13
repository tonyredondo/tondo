# Functions and closures

A function's signature is its whole contract: what it takes, what it gives,
and whether it can fail. This chapter covers declaring and calling
functions, methods, the ways a type is built, closures, and functions as
values.

- [Declaring a function](#declaring-a-function)
- [Arguments: by position, by name, by default](#arguments-by-position-by-name-by-default)
- [Any number of arguments](#any-number-of-arguments)
- [Methods](#methods)
- [Methods that change their value](#methods-that-change-their-value)
- [Building a type](#building-a-type)
- [Closures](#closures)
- [What a closure captures](#what-a-closure-captures)
- [Functions as values](#functions-as-values)
- [Taking a function: fn types and Call bounds](#taking-a-function-fn-types-and-call-bounds)
- [Recursion](#recursion)

## Declaring a function

```tondo
import std.console

fn log(message: String) {
    console.println("log: {message}")
}

fn add(a: Int, b: Int): Int {
    a + b
}

fn absolute(value: Int): Int {
    if value < 0 {
        return -value
    }
    value
}

fn main() {
    log("starting")
    console.println("{add(2, 3)} {absolute(-7)}")
}
```

```text output
log: starting
5 7
```

Parameters always state their types, and so does a result. The result
follows `:`, and there is no arrow. The four shapes of a signature are:

| Signature | Gives | Can fail |
| --- | --- | --- |
| `fn log(message: String)` | nothing (`Unit`) | no |
| `fn add(a: Int, b: Int): Int` | an `Int` | no |
| `fn save(config: Config): !IoError` | nothing | with an `IoError` |
| `fn load(path: String): Config ! IoError` | a `Config` | with an `IoError` |

The body's last expression is the result. `return value` leaves early, and
in a function that can fail, `fail error` leaves with the error
([Errors](07-errors.md)).

A parameter is a `let`: the function cannot assign to it. A function gets
its own copy of what it is given, so nothing it does to a parameter reaches
the caller, unless the parameter is lent with `ref`, `mut` or `var`
([Ownership](12-ownership.md)).

There is no overloading. One name is one function in its module, and
variations have their own names, a generic parameter, or default arguments.

## Arguments: by position, by name, by default

```tondo
import std.console

fn connect(host: String, port: Int = 443, secure: Bool = true): String {
    let scheme = if secure { "https" } else { "http" }
    "{scheme}://{host}:{port}"
}

fn main() {
    console.println(connect("example.com"))
    console.println(connect("example.com", 8443))
    console.println(connect("localhost", port: 8080, secure: false))
    console.println(connect(secure: false, host: "db", port: 5432))
}
```

```text output
https://example.com:443
https://example.com:8443
http://localhost:8080
http://db:5432
```

- Positional arguments come first. After the first named one, the rest are
  named.
- A name is the parameter's, exactly, so the parameters' names of a public
  function are part of its API.
- A default is a constant: a literal, `none`, `[]`, `[:]`, a variant or a
  record of those. A parameter with a default can be left out only after the
  last positional argument; elsewhere it is given by name.
- Arguments are evaluated in the order they are written, not in the order of
  the parameters.
- A parameter named `_` takes an argument and ignores it.

When a function has many options, give it a record with defaults
([Records](05-records.md)), so that a call says only what differs.

## Any number of arguments

The last parameter may take any number of values of one type. Inside, it is
an array.

```tondo
import std.console

fn total(label: String, values: ...Int): String {
    "{label}: {values.sum()} of {values.length()}"
}

fn main() {
    console.println(total("none"))
    console.println(total("three", 1, 2, 3))
    let more = [10, 20]
    console.println(total("spread", ...more))
    console.println(total("mixed", 1, ...more))
}
```

```text output
none: 0 of 0
three: 6 of 3
spread: 30 of 2
mixed: 31 of 3
```

`...array` spreads an array into the last position of the call.

## Methods

A method is a function declared with its type's name. It is written outside
the type, anywhere in the type's module, and called with a dot:

```tondo
import std.console
import std.math

pub type Point = { x: Float, y: Float }

/// The distance between two points.
pub fn Point.distanceTo(self, other: Point): Float {
    math.hypot(self.x - other.x, self.y - other.y)
}

/// With no `self`, a function that belongs to the type, called through it.
pub fn Point.origin(): Point {
    Point { x: 0.0, y: 0.0 }
}

fn main() {
    let home = Point.origin()
    let work = Point { x: 3.0, y: 4.0 }
    console.println("{home.distanceTo(work)}")
    console.println("{Point.distanceTo(home, work)}")      // the same call, written in full
}
```

```text output
5.0
5.0
```

`self` is the value the method is called on, lent for the call: the method
reads it and neither copies nor keeps it. A function of the type without
`self`, as `Point.origin`, is called as `Point.origin()` and never with a
dot.

A method is found without searching: `value.method()` is a method of the
value's type, or of a trait its type is bound by in generic code
([Generics and traits](11-generics.md)). A method cannot have the name of
one of its type's fields, so `value.name` is always the field.

A generic type's methods name its parameters:

```tondo
import std.console

type Pair[A, B] = { first: A, second: B }

fn Pair[A, B].swapped(self): Pair[B, A] {
    Pair { first: self.second, second: self.first }
}

fn main() {
    let pair = Pair { first: 1, second: "one" }
    let other = pair.swapped()
    console.println("{other.first} {other.second}")
}
```

```text output
one 1
```

## Methods that change their value

`self` cannot be changed. A method that changes the value it is called on
says `mut self`, and one that may also replace or resize it says `var self`:

```tondo
import std.console

type Counter = { value: Int, history: Array[Int] }

fn Counter.increment(mut self) {
    self.value += 1
}

fn Counter.record(mut self) {
    self.history.push(self.value)
}

fn Counter.reset(var self) {
    self = Counter { value: 0, history: [] }
}

fn main() {
    var counter = Counter { value: 0, history: [] }
    counter.increment()
    counter.increment()
    counter.record()
    console.println("{counter.value} {counter.history}")
    counter.reset()
    console.println("{counter.value} {counter.history}")
}
```

```text output
2 [2]
0 []
```

Such a method can only be called on something that can change: a `var`
binding, or a field or an element reached through one. On a `let` it is a
compile error. `Array.push` is a `var self` method, which is why
`values.push(4)` needs `values` to be a `var`.

## Building a type

A record is built with a literal, `Point { x: 1.0, y: 2.0 }`. When building
needs work, or the type has private fields, the type gets a constructor: a
function with the type's own name.

```tondo
import std.console

pub type Temperature = {
    priv kelvin: Float
}

/// A temperature from degrees Celsius.
pub fn Temperature(celsius: Float = 0.0): Temperature {
    Temperature { kelvin: celsius + 273.15 }
}

/// Another way to build one has its own name.
pub fn Temperature.fromFahrenheit(degrees: Float): Temperature {
    Temperature((degrees - 32.0) / 1.8)
}

pub fn Temperature.celsius(self): Float {
    self.kelvin - 273.15
}

fn main() {
    let freezing = Temperature()
    let body = Temperature.fromFahrenheit(98.6)
    console.println("{freezing.celsius():.1} {body.celsius():.1}")
}
```

```text output
0.0 37.0
```

A type is built by calling it, everywhere: `http.App()`, `sync.Mutex(0)`,
`Array[Int]()`, `Map[String, Int]()`, `UserId(42)`. There is no `new`. Since
nothing is overloaded, a type has one constructor, and the other ways to
make one are functions of the type with names that say what they start
from.

## Closures

A closure is a function written where it is used: its parameters in
parentheses, then its body. It has no `fn` and no arrow.

```tondo
import std.console

fn main() {
    let double = (value: Int): Int { value * 2 }
    let greet = (name: String) { "hello {name}" }      // the result is inferred
    let ready = () { console.println("ready") }          // no parameters: `()`
    console.println("{double(21)} {greet("Ada")}")
    ready()

    // Where a function is expected, the parameters' types come from it.
    let lengths = ["one", "three"].map((word) { word.length() })
    let short = ["one", "three"].filter((word) { word.length() < 4 })
    console.println("{lengths} {short}")
}
```

```text output
42 hello Ada
ready
[3, 5] [one]
```

The parentheses are always there, also around one parameter. A closure says
its result only when it wants to: what it gives and what it fails with are
worked out from its body, so one that holds a `fail LoadError.Missing` and
gives a `String` is a `fn(...): String ! LoadError`.

## What a closure captures

A closure uses the bindings around it. It takes them by value, at the moment
it is made, and from then on it has its own:

```tondo
import std.console

fn main() {
    let prefix = "item"
    let label = (n: Int) { "{prefix}-{n}" }
    console.println(label(1))

    var count = 0
    var next = (): Int {
        count += 1          // the closure's own count
        count
    }
    console.println("{next()} {next()} {count}")
}
```

```text output
item-1
1 2 0
```

`next` counts with its own copy of `count`, and the `count` outside is still
0. This is the same rule as everywhere else: a value given to something is a
copy. A closure never holds a hidden reference to a binding, so it can go to
another task or outlive the function that made it with nothing to check.

To change something outside, say so in one of the usual ways:

```tondo
import std.console
import std.sync

fn main() {
    // Lend the value for the call.
    var count = 0
    let increment = (value: mut Int) { value += 1 }
    increment(mut count)
    increment(mut count)

    // Or share state that is made to be shared.
    let hits = sync.Atomic(0)
    let hit = () { _ = hits.add(1) }
    hit()
    hit()
    hit()
    console.println("{count} {hits.load()}")
}
```

```text output
2 3
```

## Functions as values

A named function is a value, and so is a closure. They go in bindings,
fields and collections, and come back from functions:

```tondo
import std.console

fn shout(text: String): String {
    "{text.toUpper()}!"
}

fn compose(first: fn(String): String, second: fn(String): String): fn(String): String {
    (text: String): String { second(first(text)) }
}

type Step = { name: String, run: fn(String): String }

fn main() {
    let trim = (text: String): String { text.trim() }
    let steps = [
        Step { name: "trim", run: trim },
        Step { name: "shout", run: shout },
        Step { name: "both", run: compose(trim, shout) },
    ]
    for step in steps {
        console.println("{step.name}: [{step.run("  hello ")}]")
    }
}
```

```text output
trim: [hello]
shout: [  HELLO !]
both: [HELLO!]
```

`fn(String): String` is the type of any function that takes a `String` and
gives one. The whole signature is in the type: `fn(Int, Int): Bool`,
`fn(String): Config ! LoadError`, `fn(mut Array[Float], Float)`. Parameter
names are not, so a call through a function value gives its arguments by
position.

A generic function becomes a value once its types are known,
`identity[Int]`, or when what is expected leaves one answer.

What builds a value is a function too, and is given by its name where a
function is expected: a method by its type, a function of a type as
`Int.parse`, a variant with data, a newtype, and `some`, `ok` and `err`. No
closure is needed only to wrap or to forward:

```tondo
import std.console
import std.text

type UserId = Int

enum LoadError {
    BadNumber(text.TextError)
}

fn userId(written: String): UserId ! LoadError {
    Int.parse(written).map(UserId).mapErr(LoadError.BadNumber)
}

fn main() {
    let names = ["ada", "bo"].map(String.toUpper)
    let lengths = ["ada", "bo"].map(String.length)
    let ids = [7, 8].map(UserId)
    let found: Array[Int?] = [1, 2].map(some)
    console.println("{names} {lengths} {ids.map((id) { id.value })} {found}")
    console.println("{["4", "x"].map(Int.parse)}")
    match userId("x") {
        ok(id) => console.println("{id.value}")
        err(error) => console.println("{error}")
    }
}
```

```text output
[ADA, BO] [3, 2] [7, 8] [some(1), some(2)]
[ok(4), err(invalid data)]
BadNumber(invalid data)
```

`String.toUpper` is the function that takes a `String` and calls `toUpper`
on it. A function value has no defaults, so a method that has them is
given whole or with a closure: `String.trim` takes the text and what to
trim, and `(name) { name.trim() }` the text alone. A method bound to one
value, `value.method` with no call, is still not a value: a closure does
that job, `(x) { value.method(x) }`.

## Taking a function: fn types and Call bounds

A function that receives another can ask for it in two ways.

```tondo
import std.console

/// Any function of this signature, kept as a value.
fn apply(value: Int, operation: fn(Int): Int): Int {
    operation(value)
}

/// Anything that can be called so, compiled for each one it is given.
fn applyTwice[F: Call[fn(Int): Int]](value: Int, operation: F): Int {
    operation(operation(value))
}

/// One that changes what it captured each time it is called.
fn collect[F: CallMut[fn(): Int]](times: Int, next: F): Array[Int] {
    var step = next
    var results: Array[Int] = []
    for _ in 0..times {
        results.push(step())
    }
    results
}

fn main() {
    var count = 0
    let next = (): Int {
        count += 10
        count
    }
    console.println("{apply(5, (x) { x + 1 })} {applyTwice(5, (x) { x * 3 })} {collect(3, next)}")
}
```

```text output
6 45 [10, 20, 30]
```

- `fn(Int): Int` is one uniform type. Use it to store a callback in a
  record or a collection.
- `F: Call[fn(Int): Int]` takes the closure as the exact thing it is, with
  what it captured, and the call is as direct as if it were written in
  place. Use it for a function that only calls what it is given.
- `CallMut` is for a closure that writes to its captures, and `CallOnce` for
  one that gives a capture away and so can run once. A `Call` is also a
  `CallMut`, and a `CallMut` a `CallOnce`, so asking for the weakest one a
  function needs accepts the most.

The compiler works out which of the three a closure is from its body. The
methods of the standard library are written this way, which is why
`numbers.map((n) { n * 2 })` costs what the loop would.

## Recursion

A function may call itself, and the functions of a module may call each
other in any order: all signatures are known before any body is checked.

```tondo
import std.console

fn isEven(n: Int): Bool {
    if n == 0 { true } else { isOdd(n - 1) }
}

fn isOdd(n: Int): Bool {
    if n == 0 { false } else { isEven(n - 1) }
}

fn factorial(n: Int): Int {
    if n <= 1 { 1 } else { n * factorial(n - 1) }
}

fn main() {
    console.println("{isEven(10)} {isOdd(7)} {factorial(20)}")
}
```

```text output
true true 2432902008176640000
```

A call is not promised to reuse its caller's frame, so an algorithm whose
depth grows with its input is better written as a loop. A recursion that
runs out of stack stops the program with a report, as any other fault does.

Next: [Control flow](04-control-flow.md).
