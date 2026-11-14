# Values, bindings and types

Everything in Tondo is a value with one type, known before the program runs.
This chapter is about naming values, the types every program starts from,
and how one type becomes another.

- [let and var](#let-and-var)
- [Taking a value apart](#taking-a-value-apart)
- [A name is used once](#a-name-is-used-once)
- [Constants](#constants)
- [The types every program has](#the-types-every-program-has)
- [Literals](#literals)
- [What is inferred, and what is written](#what-is-inferred-and-what-is-written)
- [Conversions](#conversions)
- [Unit and Never](#unit-and-never)
- [Other names for a type, and new types](#other-names-for-a-type-and-new-types)
- [Equality](#equality)

## let and var

`let` names a value that does not change. `var` names one that does.

```tondo
import std.console

fn main() {
    let name = "Ada"            // its type, String, comes from the value
    let limit: Int = 10         // or is written
    var total = 0
    for n in 1..=limit {
        total += n
    }
    console.println("{name}: {total}")
}
```

```text output
Ada: 55
```

A binding always starts with a value: there is no declaration to fill in
later, so nothing is ever uninitialized.

```text
error[E1109]: a binding must be initialized where it is declared
```

`let` is deep. Through a `let` binding, neither the binding nor anything
inside its value can be changed: not a field of a record, not an element of
an array. `var` allows both. Mutability belongs to the binding, not to the
type, so the same `Array[Int]` is fixed in one place and growing in another,
and a copy of a `let` value into a `var` is a new value to change freely
([Ownership](12-ownership.md)).

```tondo
import std.console

type Counter = { hits: Int }

fn main() {
    let fixed = [1, 2, 3]
    var growing = fixed         // a copy
    growing.push(4)
    var counter = Counter { hits: 0 }
    counter.hits += 1
    console.println("{fixed} {growing} {counter.hits}")
}
```

```text output
[1, 2, 3] [1, 2, 3, 4] 1
```

## Taking a value apart

A binding can be a pattern that always matches: a tuple, or a record by its
fields.

```tondo
import std.console

type Point = { x: Int, y: Int }

fn bounds(values: Array[Int]): (Int, Int) {
    (values.min() ?? 0, values.max() ?? 0)
}

fn main() {
    let (low, high) = bounds([4, 9, 2])
    let Point { x, y } = Point { x: 3, y: 4 }
    var (a, b) = (1, 2)
    (a, b) = (b, a)             // both sides at once: a swap
    console.println("{low} {high} {x} {y} {a} {b}")
}
```

```text output
2 9 3 4 2 1
```

A pattern that may not match, as one variant of an enum, belongs in a
`match` ([Enums and match](06-enums.md)).

## A name is used once

A binding cannot hide another that is still visible. This is on purpose:
when a name appears, there is one place it can come from.

```tondo error E1003
fn main() {
    let value = "10"
    if value.length() > 1 {
        let value = 10
        _ = value
    }
}
```

```text
error[E1003]: `value` would shadow a visible binding with the same name: choose another name, or assign `value = ...` if it is a `var`
```

The fix is a name that says what changed, `rawValue` and `value`, or
`text` and `number`. Two blocks side by side may each use a name, since
neither sees the other, and `_` can be written any number of times. A
parameter, a loop's variable and a closure's parameter are bindings too, and
none may hide an import or a function of the file.

## Constants

```tondo
import std.console

const MaxRetries: Int = 3
const Host = "127.0.0.1"
const Limits: Array[Int] = [10, 100, 1_000]
pub const Version: String = "1.4.0"

fn main() {
    console.println("{Host} {MaxRetries} {Limits} {Version}")
}
```

```text output
127.0.0.1 3 [10, 100, 1000] 1.4.0
```

A constant belongs to its module and is computed when the program compiles.
It is made of literals, other constants, operators on them, and tuples,
records, enums and collections of those. It cannot call the program's
functions, read a file, the clock or the environment, with one exception:
`embed.text`, `embed.bytes` and `embed.files` read files of the package while
it compiles, which is how a page or a migration ends up inside the
executable.

A `pub` constant states its type, as everything public does. An expression
that would panic, as a division by zero, is a compile error in a constant.

There are no global variables. What a whole program shares and changes is a
field of a [context](14-contexts.md), or state in `std.sync`.

## The types every program has

| Type | What it is |
| --- | --- |
| `Bool` | `true` or `false`; nothing else is a condition |
| `Int` | a signed integer of 64 bits, on every machine |
| `UInt` | an unsigned integer of 64 bits |
| `Float` | an IEEE 754 number of 64 bits |
| `Char` | one Unicode scalar: `'a'`, `'ñ'`, `'λ'` |
| `Byte` | one byte of data; it has no arithmetic |
| `String` | text: a sequence of characters, UTF-8 inside, never changed in place |
| `Unit` | the type of `()`: nothing to say |
| `Never` | the type of what does not come back |

Sized numbers, for a binary format or for C:

```text
Int8  Int16  Int32  Int64  Int128
UInt8 UInt16 UInt32 UInt64 UInt128
Float32 Float64
```

`Int` and `Int64` are one type under two names, as are `UInt` and `UInt64`,
and `Float` and `Float64`. Every width has the same operations
([Numbers](10-numbers.md)).

On these the rest is built: [records, tuples and
newtypes](05-records.md), [enums and unions](06-enums.md), `T?` and `T ! E`
([Errors](07-errors.md)), and the [collections](08-collections.md).

## Literals

```tondo
import std.console

fn main() {
    let decimal = 1_000_000
    let hex = 0xFF_A0
    let binary = 0b1010_0110
    let octal = 0o755
    let small = 42u8                // a suffix fixes the type: i8 to i128, u8 to u128
    let byte: Byte = 0x41           // a literal from 0 to 255 where a Byte is expected
    let avogadro = 6.022e23
    let half: Float32 = 0.5
    let letter = 'λ'
    let smile = '\u{1F642}'
    console.println("{decimal} {hex} {binary} {octal} {small} {byte}")
    console.println("{avogadro} {half} {letter} {smile}")
}
```

```text output
1000000 65440 166 493 42 65
6.022e23 0.5 λ 🙂
```

- An integer literal is an `Int`, unless where it goes expects another
  integer type and its value fits; one that does not fit is a compile error.
- A float literal has a decimal point or an exponent. `1` is never a
  `Float`: write `1.0`.
- `_` may separate digits. A decimal number does not start with `0`, so
  `007` is an error rather than an octal surprise.
- A `Char` is exactly one scalar. Its escapes are `\n`, `\r`, `\t`, `\\`,
  `\'`, `\0` and `\u{HEX}`. A letter with a combining accent is two
  scalars, and so a `String`.
- Strings have their own chapter, with interpolation, raw and multi-line
  forms: [Text](09-text.md).

Collections have literals too: `[1, 2, 3]` is an array, `["a": 1]` a map,
`[:]` the empty map, `Set[1, 2]` a set ([Collections](08-collections.md)).

## What is inferred, and what is written

Inside a function the compiler works types out. Between functions they are
written, so a signature is the whole contract and reading one function never
requires reading another.

Inferred:

- the type of a `let` or `var` from its value;
- the type arguments of a generic call from its arguments and from what is
  expected of its result;
- a literal's type from where it goes;
- a closure's parameter and result types from the function it is passed to.

Written:

- the parameters and the result of a named function;
- the type of a `pub` constant;
- an empty collection that nothing gives a type to;
- a union of types: the compiler never invents one.

```tondo
import std.console

fn main() {
    let numbers = [1, 2, 3]                     // Array[Int]
    let empty: Array[String] = []               // nothing to infer from
    let mixed: Array[Int | String] = [1, "a"]   // a union is asked for
    let doubled = numbers.map((n) { n * 2 })    // the closure's types come from map
    console.println("{numbers} {empty} {mixed} {doubled}")
}
```

```text output
[1, 2, 3] [] [1, a] [2, 4, 6]
```

## Conversions

A number becomes another without any syntax only when no value can be lost:
an integer into a wider integer, and `Float32` into `Float`.

```tondo
import std.console

fn main() {
    let small: Int32 = 70_000
    let wide: Int = small           // widens
    let total = small + wide        // an Int
    console.println("{total}")
}
```

```text output
140000
```

Everything else is asked for, by calling the type. A conversion that cannot
fail gives the value; one that can gives a result, and so cannot be ignored:

```tondo
import std.console

fn main() {
    let count = 10
    let ratio = Float(count) / 4.0          // an Int always has a Float
    let whole = Int(3.0)                    // a Float may not have an Int
    let broken = Int(3.7)
    let tiny = Int8(300)
    console.println("{ratio} {whole} {broken} {tiny}")
    console.println("{UInt8.wrapping(300)} {UInt8.saturating(300)} {Int8(100) ?? 0}")
}
```

```text output
2.5 ok(3) err(NotIntegral) err(OutOfRange)
44 255 100
```

- `Float(n)` is exact up to 2⁵³ and rounds beyond.
- `Int(x)`, `Int8(n)`, `Byte(n)` and every narrowing give a
  `T ! NumericConversionError`, whose variants are `OutOfRange`,
  `NotFinite` and `NotIntegral`. Use `?`, `match` or `??`.
- To keep the low bits or the nearest value on purpose, say so:
  `UInt8.wrapping(n)`, `UInt8.saturating(n)`.
- Nothing converts between `Bool` and numbers, between a number and its
  text (`Int.parse("42")`, `"{n}"`), or between a type and its newtype.

Where a variable does not fit by its type, the error gives the conversion to
write:

```text
error[E1102]: expected `Int32`, found `Int`, which may not fit: convert it with `Int32(n)?`, or handle the error with `match`
error[E1102]: expected `Float`, found `Int`: convert it with `Float(n)`
```

One more thing is done for you: where a `T?` is expected, a plain `T` is
taken as `some` of it, so `let limit: Int? = 5` holds `some(5)`.

## Unit and Never

`Unit` is the type of `()`, the value of a function that has nothing to
give back. A function with no result type returns it, and so does a block
whose last statement is not an expression.

`Never` is the type of an expression that does not finish: `panic(...)`,
`process.exit(...)`, `return`, `fail`, `break`. It fits wherever any type is
expected, which is what lets one branch of an `if` give a value and the
other leave:

```tondo
import std.console

fn half(n: Int): Int {
    let even = if n % 2 == 0 { n } else { panic("{n} is odd") }
    even / 2
}

fn main() {
    console.println("{half(10)}")
}
```

```text output
5
```

## Other names for a type, and new types

`alias` gives a type another name. The two are the same type and mix freely.
`type` over an existing type makes a new one, which the compiler never
confuses with the first: that is the cheapest way to stop a user's
identifier from being passed where an order's goes.

```tondo
import std.console

alias Index = Int
pub type UserId = Int
pub type OrderId = Int

fn find(user: UserId): String {
    "user {user.value}"
}

fn main() {
    let position: Index = 3
    let next = position + 1         // an Index is an Int
    let user = UserId(42)           // built by calling the type
    let order = OrderId(42)
    console.println("{next} {find(user)} {order.value}")
}
```

```text output
4 user 42 42
```

`find(order)` would not compile, and neither would `find(42)`. `.value`
gives back what a newtype holds. A newtype keeps what its value can do by
nature, being copied, compared, used as a key, sent to a task, and none of
its methods: `UserId` has no arithmetic unless the program gives it some.
[Records](05-records.md) has more.

## Equality

`==` and `!=` compare values, whatever they are: two records with equal
fields are equal, two arrays with equal elements in the same order, two
strings with the same characters. There is no second operator for
"the same object", because values have no identity. What needs one is a
`Ref[T]` ([Ownership](12-ownership.md)).

```tondo
import std.console

type Point = { x: Int, y: Int }

fn main() {
    console.println("{Point { x: 1, y: 2 } == Point { x: 1, y: 2 }}")
    console.println("{[1, 2] == [1, 2]} {(1, "a") == (1, "b")} {"año" == "año"}")
}
```

```text output
true
true false true
```

A type can be compared when all its parts can, and the compiler works that
out; functions and a few handles to outside resources cannot. Both sides of
`==` have the same type: `1 == 1.0` is an error, not `false`.

Next: [Functions and closures](03-functions.md).
