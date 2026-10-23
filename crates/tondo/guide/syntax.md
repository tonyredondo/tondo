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
}
```

`for` is the only loop. `if` is an expression; its condition is a `Bool`:
there is no truthiness (`if list.isEmpty()`, not `if list`).

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
    console.println("{area(Shape.Rect { width: 2.0, height: 3.0 })} {describe(some(12))}")
    let first = match [1, 2, 3] {
        [head, ..] => head
        [] => 0
    }
    console.println("{first}")
}
```

A match is exhaustive: a missing case is a compile error that names it. One
arm per line; `_` matches anything. For several values write several arms
(there is no `|` in patterns).

## Closures and function values

```tondo
import std.console

fn twice(f: fn(Int): Int, x: Int): Int {
    f(f(x))
}

fn main() {
    let double = (n: Int): Int { n * 2 }
    console.println("{twice(double, 3)}")
    console.println("{twice((n) { n + 1 }, 3)}")      // types from the context
    let squares = [1, 2, 3].map((n) { n * n })
    console.println("{squares}")
}
```

A closure is `(params) { body }`, with parameter types when the context does
not give them. A closure that fails declares its result:
`(): String ! LoadError { ... }`.

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
