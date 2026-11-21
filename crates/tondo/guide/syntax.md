# Syntax

## Bindings and functions

```tondo
import std.console

const Limit: Int = 10

fn add(a: Int, b: Int): Int {
    a + b
}

/// Named arguments and defaults: `greet("Ada", greeting: "Hi")`.
fn greet(name: String, greeting: String = "Hello"): String {
    "{greeting}, {name}"
}

/// A variadic parameter is an array inside.
fn total(values: ...Int): Int {
    var sum = 0
    for value in values {
        sum += value
    }
    sum
}

fn main() {
    let x = add(2, 3)               // immutable
    var count = 0                   // mutable
    count += x
    let (first, second) = (1, "two")
    console.println("{x} {count} {first} {second} {Limit}")
    console.println(greet("Ada", greeting: "Hi"))
    console.println("{total(1, 2, 3)}")
}
```

- Types are inferred inside functions; signatures always state them.
- A function's last expression is its value; `return value` leaves early.
- A public constant states its type: `pub const Max: Int = 10`.

## Control flow

```tondo
import std.console

fn sign(n: Int): String {
    if n > 0 { "positive" } else if n < 0 { "negative" } else { "zero" }
}

fn main() {
    for i in 0..3 {                 // 0, 1, 2; `0..=3` includes 3
        console.println("{i} {sign(i - 1)}")
    }
    var n = 10
    for n > 1 {                     // a loop with a condition
        n = n / 2
    }
    for {                           // a loop until `break`
        n += 1
        if n == 5 {
            break
        }
    }
    for item in ["a", "b"] {
        if item == "a" {
            continue
        }
        console.println(item)
    }
    let odd = for i in 0..6 {       // where a value is wanted, the array of
        if i % 2 == 0 {             // what each turn ends with: [1, 9, 25]
            continue                // this turn gives nothing
        }
        i * i
    }
    console.println("{odd}")
}
```

`for` is the only loop. `if` is an expression; its condition is a `Bool`:
there is no truthiness (`if list.isEmpty()`, not `if list`).

A `for` gives its values on the right of `let` or `=`, in an argument, after
`return`, and at the end of a block that has to give a value: the body of a
function with a result type, a branch or an arm whose value is used, a turn
of another such loop. `continue` leaves a turn's element out and `break`
ends with what was given. Anywhere else it is a statement, as at the end of
a closure's block whose result nothing states, the one `map` takes: there
write the arrow, whose body is a value, `rows.map(row => for x in row { x * 10 })`,
or `return for ... { ... }` after the block's statements. `map` stays the short form of one expression; the
loop is for a turn of several lines, or one that can fail, since a `?` in it
leaves the function: `for result in results { result? }` gives the values
or fails with the first error. A turn of such a loop ends with its value:

```tondo error E1623
import std.console

fn main() {
    let printed = for name in ["ada", "bo"] {
        console.println(name)
    }
    _ = printed
}
```

## match

```tondo
import std.console

enum Shape {
    Circle(Float)
    Rect { width: Float, height: Float }
    Point
}

fn area(shape: Shape): Float {
    match shape {
        Shape.Circle(radius) => 3.14159 * radius * radius
        Shape.Rect { width, height } => width * height
        Shape.Point => 0.0
    }
}

fn describe(n: Int?): String {
    match n {
        some(value) if value > 9 => "big {value}"
        some(value) => "small {value}"
        none => "nothing"
    }
}

fn main() {
    console.println("{area(Shape.Rect { width: 2.0, height: 3.0 })} {describe(12)}")
    let first = match [1, 2, 3] {
        [head, ..] => head
        [] => 0
    }
    console.println("{first}")
}
```

A match is exhaustive: a missing case is a compile error that names it. One
arm per line; `_` matches anything. `1 | 2 =>` joins patterns that bind
nothing in one arm. Where the value is of an enum, a pattern names a variant
alone, at any depth: `some(Card(number))` for `some(Payment.Card(number))`.

## Closures and function values

```tondo
import std.console

fn twice(f: fn(Int): Int, x: Int): Int {
    f(f(x))
}

fn main() {
    let double = (n: Int): Int { n * 2 }
    console.println("{twice(double, 3)}")
    console.println("{twice(n => n + 1, 3)}")      // types from the context
    let squares = [1, 2, 3].map(n => n * n)
    console.println("{squares}")
}
```

A closure of one expression is an arrow, `n => n + 1`, `(a, b) => a + b`,
`() => clock.now()`: one parameter stands alone, none or several go in
parentheses, and the types come from where it is passed. One with
statements, or with a type, a `mut`, `ref` or `var`, or a result written,
is a block with no arrow, `(n: Int): Int { ... }`, whose parentheses are
always there; `x => { ... }` and `(x: Int) => ...` are refused, and `tondo
fmt` writes a block of one expression as an arrow. Where no function type
is expected, as in a `let`, a closure needs its parameters' types, so the
block, and it infers its result, and what it fails with from its `fail` and
`?`: a closure that fails with `fail LoadError.Missing` and gives a `String`
is a `fn(...): String ! LoadError`. Where a function is expected, a method, a
variant with data, a newtype and `some` are given by name:
`names.map(String.toUpper)`, `result.mapErr(AppError.Io)`.

## defer

```tondo
import std.console

fn work() {
    defer console.println("cleanup")    // runs when the block ends, however
    console.println("working")
}

fn main() {
    work()
}
```
