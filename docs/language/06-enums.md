# Enums, unions and match

A record holds several values together. An enum holds one of several
alternatives, each with its own data, and `match` is how a program asks
which. Together they let a type say exactly what can be, so that what cannot
be does not compile.

- [Enums](#enums)
- [match](#match)
- [Every case is covered](#every-case-is-covered)
- [Patterns](#patterns)
- [Guards](#guards)
- [Arrays in patterns](#arrays-in-patterns)
- [Recursive types](#recursive-types)
- [Unions: one of several types](#unions-one-of-several-types)
- [Enum or union](#enum-or-union)
- [Looking without taking](#looking-without-taking)
- [Modelling with enums](#modelling-with-enums)

## Enums

```tondo
import std.console

pub enum Payment {
    Cash
    Card(String)
    Transfer { iban: String, reference: String }
}

fn main() {
    let payments = [
        Payment.Cash,
        Payment.Card("4242"),
        Payment.Transfer { iban: "ES91 2100", reference: "invoice 7" },
    ]
    console.println("{payments.length()} payments, {payments[0] == Payment.Cash}")
}
```

```text output
3 payments, true
```

A variant has no data, data by position, or data by name. To make a value
it is written with its enum, `Payment.Card("4242")`: there is nothing to
import to shorten it, and so a variant is never confused with a type or a
function. Where the enum is already known, in a pattern and after `fail`, it
is written alone, as the next section shows.

An enum is a value like any other: it is copied, compared with `==`, used as
a map's key when its data can be, and it has methods. It is also shown with
no `Display` to write: a variant by its name, and with its data when it has
any, `Card(4242)`.

```tondo
import std.console

pub enum Level {
    Debug
    Info
    Warning
    Error
}

fn Level.isSerious(self): Bool {
    self == Level.Warning or self == Level.Error
}

fn main() {
    let level = Level.Warning
    var counts: Map[Level, Int] = [:]
    counts[level, default: 0] += 1
    console.println("{level} {level.isSerious()} {counts}")
}
```

```text output
Warning true [Warning: 1]
```

## match

`match` takes a value and runs the first arm whose pattern fits it. A
pattern names the parts of the value it wants:

```tondo
import std.console

pub enum Payment {
    Cash
    Card(String)
    Transfer { iban: String, reference: String }
}

fn describe(payment: Payment): String {
    match payment {
        Cash => "cash"
        Card(number) => "card ending in {number}"
        Transfer { iban, reference } => "transfer from {iban} ({reference})"
    }
}

fn fee(payment: Payment, amount: Int): Int {
    match payment {
        Cash => 0
        Card(_) => {
            let percent = amount * 2 / 100
            percent.max(30)
        }
        Transfer { .. } => 25
    }
}

fn main() {
    let payment = Payment.Card("4242")
    console.println("{describe(payment)}, fee {fee(payment, 1000)}")
}
```

```text output
card ending in 4242, fee 30
```

A pattern names a variant alone, `Card(number)`: the value being matched is
a `Payment`, so there is nothing else `Card` could be. `Payment.Card(number)`
says the same and is also accepted. This holds at any depth, as in
`some(Card(number))`.

Several patterns share an arm with `|`, when none of them binds a name:

```tondo
import std.console

enum Token {
    Plus
    Minus
    Star
    Number(Int)
    Word(String)
}

fn kind(token: Token): String {
    match token {
        Token.Plus | Token.Minus | Token.Star => "an operator"
        Number(_) | Word(_) => "an operand"
    }
}

fn main() {
    console.println("{kind(Token.Star)}, {kind(Token.Number(4))}")
    console.println("{Token.Number(4)} {Token.Plus} {[Token.Word("tea"), Token.Minus]}")
}
```

```text output
an operator, an operand
Number(4) Plus [Word(tea), Minus]
```

A pattern that binds a name takes an arm of its own, so that the name always
has a value:

```text
error[E1202]: the patterns that `|` joins bind nothing, and this one binds `n`: write an arm of its own for it
```

`match` is an expression, and its arms give the same type. An arm is one
expression, a block, or one of `return`, `fail`, `break`, `continue` or an
assignment, which is handy for the arm that leaves:

```tondo
import std.console

fn firstNumber(words: Array[String]): Int? {
    for word in words {
        match Int.parse(word) {
            ok(number) => return number
            err(_) => continue
        }
    }
    none
}

fn main() {
    console.println("{firstNumber(["a", "12", "7"])} {firstNumber(["a"])}")
}
```

```text output
some(12) none
```

## Every case is covered

A `match` must cover every value its subject can have, and the compiler
names what is missing:

```tondo error E1204
enum Light {
    Red
    Amber
    Green
}

fn next(light: Light): Light {
    match light {
        Red => Light.Green
        Green => Light.Amber
    }
}

fn main() {
    _ = next(Light.Red)
}
```

```text
error[E1204]: `match` is not exhaustive: `Light.Amber` is not covered
```

This is the point of an enum. Add a variant, and every `match` that does not
handle it stops compiling: the compiler walks the program to each place that
has to decide what the new case means. A `_` arm gives that up, since it
takes whatever is left, so use it for subjects that have no end, as numbers
and strings, and name the variants of an enum.

An arm that can never run, because the arms above take everything it would,
is an error too:

```text
error[E1203]: this arm is covered by earlier arms
```

## Patterns

| Pattern | Fits | Binds |
| --- | --- | --- |
| `_` | anything | nothing |
| `name` | anything | the value, as `name` |
| `0`, `"quit"`, `'a'`, `true`, `none` | that value | nothing |
| `(a, b)` | a tuple, part by part | what its parts bind |
| `User { name, age }` | a record | its fields, by their names |
| `User { name: who, .. }` | a record | `name` as `who`; `..` skips the rest |
| `Circle(radius)`, `Shape.Circle(radius)` | that variant | its data |
| `Rect { width, height }` | that variant | its fields |
| `Red \| Amber` | either pattern, when neither binds a name | nothing |
| `some(value)`, `none` | an option | the value |
| `ok(value)`, `err(error)` | a result | the value, or the error |
| `Int(number)`, `String(text)` | that member of a union | the value, with that type |
| `[]`, `[a, b]`, `[first, ..rest]` | an array of that shape | its elements |

Patterns nest to any depth:

```tondo
import std.console

type Address = { city: String, country: String }
type User = { name: String, address: Address? }

enum Event {
    SignedUp(User)
    Moved { user: User, to: Address }
    Left(String)
}

fn summary(event: Event): String {
    match event {
        SignedUp(User { name, address: some(Address { city, .. }) }) => "{name} joined from {city}"
        SignedUp(User { name, address: none }) => "{name} joined"
        Moved { user, to: Address { country: "ES", city } } => "{user.name} moved to {city}, in Spain"
        Moved { user, to } => "{user.name} moved to {to.city}"
        Left(name) => "{name} left"
    }
}

fn main() {
    let madrid = Address { city: "Madrid", country: "ES" }
    let ada = User { name: "Ada", address: madrid }
    console.println(summary(Event.SignedUp(ada)))
    console.println(summary(Event.Moved { user: ada, to: madrid }))
    console.println(summary(Event.Left("Bo")))
}
```

```text output
Ada joined from Madrid
Ada moved to Madrid, in Spain
Bo left
```

Literals match exactly, with no conversion. An arm has one pattern: there is
no `|` between alternatives, so two cases that do the same have two arms, or
share a function.

## Guards

An arm can add a condition, which is checked once its pattern fits:

```tondo
import std.console

fn classify(point: (Int, Int)): String {
    match point {
        (0, 0) => "the origin"
        (x, 0) => "on the x axis, at {x}"
        (0, y) => "on the y axis, at {y}"
        (x, y) if x == y => "on the diagonal"
        (x, y) if x < 0 or y < 0 => "outside the first quadrant"
        _ => "somewhere else"
    }
}

fn main() {
    for point in [(0, 0), (3, 0), (2, 2), (-1, 5), (1, 5)] {
        console.println(classify(point))
    }
}
```

```text output
the origin
on the x axis, at 3
on the diagonal
outside the first quadrant
somewhere else
```

Arms are tried from the top, and a guard may be false, so an arm with a
guard never counts toward covering every case.

## Arrays in patterns

An array pattern fits by length. `..rest` takes what is left:

```tondo
import std.console

fn describe(values: Array[Int]): String {
    match values {
        [] => "empty"
        [only] => "just {only}"
        [first, second] => "{first} and {second}"
        [first, ..rest] => "{first} and {rest.length()} more"
    }
}

fn total(values: Array[Int]): Int {
    match values {
        [] => 0
        [head, ..tail] => head + total(tail)
    }
}

fn main() {
    console.println("{describe([])} | {describe([7])} | {describe([1, 2])} | {describe([1, 2, 3, 4])}")
    console.println("{total([1, 2, 3, 4])}")
}
```

```text output
empty | just 7 | 1 and 2 | 1 and 3 more
10
```

This is also how a program reads its arguments:

```tondo
import std.console
import std.env

fn main() {
    match env.args() {
        ["add", name] => console.println("adding {name}")
        ["remove", name, ..] => console.println("removing {name}")
        ["list"] => console.println("listing")
        _ => console.println("usage: add <name> | remove <name> | list")
    }
}
```

Maps have no patterns: ask them with `get`.

## Recursive types

An enum may contain itself, with nothing to wrap it in:

```tondo
import std.console

enum Expr {
    Number(Float)
    Add(Expr, Expr)
    Multiply(Expr, Expr)
    Negate(Expr)
}

fn evaluate(expr: Expr): Float {
    match expr {
        Number(value) => value
        Add(left, right) => evaluate(left) + evaluate(right)
        Multiply(left, right) => evaluate(left) * evaluate(right)
        Negate(inner) => -evaluate(inner)
    }
}

fn show(expr: Expr): String {
    match expr {
        Number(value) => "{value}"
        Add(left, right) => "({show(left)} + {show(right)})"
        Multiply(left, right) => "{show(left)} * {show(right)}"
        Negate(inner) => "-{show(inner)}"
    }
}

fn main() {
    let expr = Expr.Add(
        Expr.Number(2.0),
        Expr.Multiply(Expr.Number(3.0), Expr.Negate(Expr.Number(4.0))),
    )
    console.println("{show(expr)} = {evaluate(expr)}")
}
```

```text output
(2.0 + 3.0 * -4.0) = -10.0
```

A record can hold itself the same way, through an option or a collection:
`type Node = { value: Int, children: Array[Node] }`. Such values are trees.
A graph, where two nodes point at the same third, needs identity: `Ref[T]`
([Ownership](12-ownership.md)).

## Unions: one of several types

`A | B` is a value that is an `A` or a `B`. It needs no declaration, and a
value of either type goes in without wrapping:

```tondo
import std.console

fn describe(value: Int | String | Bool): String {
    match value {
        Int(number) => "the number {number}"
        String(text) => "the text {text}"
        Bool(flag) => if flag { "yes" } else { "no" }
    }
}

fn main() {
    console.println(describe(4))
    console.println(describe("four"))
    console.println(describe(true))

    let mixed: Array[Int | String] = [1, "two", 3]
    console.println("{mixed}")
}
```

```text output
the number 4
the text four
yes
[1, two, 3]
```

A union does not depend on the order it is written in, `Int | String` is
`String | Int`, and a union of unions is flat. Its members are types with
names; a tuple or a function goes in a newtype first.

The commonest union is of errors: a function that can fail in two ways says
`!(ParseError | IoError)`, and `?` puts either in
([Errors](07-errors.md)). The formats of the library read and write a union
as the member it holds, so `Map[String, Int | String]` is a JSON object of
mixed values.

## Enum or union

Both hold one of several things. They differ in who names the cases:

| | Enum | Union |
| --- | --- | --- |
| Declared | once, with a name | where it is used |
| A case is | a variant, which may hold any data | an existing type |
| Two cases with the same data | are different variants | cannot be: a type appears once |
| Best for | a public API, and states that mean something | a few existing types, inside a module |

Prefer an enum in what a module exports: it names the idea, its cases can be
renamed and documented, and it does not leak which types are behind it.

## Looking without taking

A `match` on a value that is costly or cannot be copied can look at it
without taking it apart. `ref` in a pattern lends a part for the length of
the arm:

```tondo
import std.console

enum Document {
    Draft(Array[String])
    Published { lines: Array[String], views: Int }
}

fn lineCount(document: Document): Int {
    match document {
        Draft(ref lines) => lines.length()
        Published { ref lines, .. } => lines.length()
    }
}

fn main() {
    let document = Document.Published { lines: ["one", "two"], views: 10 }
    console.println("{lineCount(document)}")
}
```

```text output
2
```

For values that can be copied, which is nearly all of them, this is only a
matter of speed. It matters for the few that cannot
([Ownership](12-ownership.md)).

## Modelling with enums

The habit to form is to give each state the data it has, and no other. A
record with a status and optional fields allows combinations that mean
nothing:

```tondo
type Connection = {
    status: String          // "idle", "connected" or "failed"
    socket: Int?            // only when connected
    error: String?          // only when failed
    retries: Int            // only when failed
}

fn main() {}
```

Nothing stops a `Connection` that is `"idle"` with a socket and an error.
The enum has no such value:

```tondo
import std.console

enum Connection {
    Idle
    Connected { socket: Int }
    Failed { error: String, retries: Int }
}

fn next(connection: Connection): Connection {
    match connection {
        Idle => Connection.Connected { socket: 3 }
        Connected { .. } => connection
        Failed { retries, .. } if retries < 3 => Connection.Idle
        Failed { .. } => connection
    }
}

fn describe(connection: Connection): String {
    match connection {
        Idle => "idle"
        Connected { socket } => "connected on {socket}"
        Failed { error, retries } => "failed {retries} times: {error}"
    }
}

fn main() {
    let failed = Connection.Failed { error: "timeout", retries: 1 }
    console.println("{describe(failed)} -> {describe(next(failed))} -> {describe(next(next(failed)))}")
}
```

```text output
failed 1 times: timeout -> idle -> connected on 3
```

A function such as `next`, which gives the following state, is also
something the toolchain can check in every state it can reach, with no model
to write (`tondo guide models`).

Next: [Errors](07-errors.md).
