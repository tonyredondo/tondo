# Types

## Scalars

`Int` (64-bit, overflow panics), `Int8`…`UInt64`, `Float` (64-bit),
`Float32`, `Bool`, `Char` (a Unicode scalar), `Byte`, `String`, `Unit`
(`()`), `Never`. Conversions are explicit: `Float(n)` cannot fail; `Int(x)`,
`Byte(n)` and other narrowing ones return `T ! NumericConversionError`.
A `Byte` literal is `Byte(255u8)`.

## Records, newtypes and aliases

```tondo
import std.console

/// A record: named fields; `priv` hides one from other modules.
pub type User = {
    name: String
    age: Int
    priv password: String
}

/// A newtype: a distinct type over another, built with `UserId(7)`.
pub type UserId = Int

/// An alias: another name for the same type.
alias Pair = (Int, String)

/// A static function and a method, declared outside the type.
fn User.new(name: String): User {
    User { name, age: 0, password: "" }
}

fn User.describe(self): String {
    "{self.name} ({self.age})"
}

/// `mut self` changes the receiver's fields.
fn User.birthday(mut self) {
    self.age += 1
}

fn main() {
    var ada = User.new("Ada")
    ada.birthday()
    let older = ada with { age: 40 }            // a copy with changes
    let id = UserId(7)
    let pair: Pair = (id.value, "seven")
    console.println("{ada.describe()} {older.age} {pair.1}")
}
```

Values are copied on assignment (collections and strings share storage until
changed). A record with a `priv` field cannot be built outside its module:
give it a constructor function.

## Enums

```tondo
import std.console

pub enum Payment {
    Cash
    Card(String)
    Transfer { iban: String, reference: String }
}

fn label(payment: Payment): String {
    match payment {
        Payment.Cash => "cash"
        Payment.Card(number) => "card {number}"
        Payment.Transfer { iban, reference } => "transfer {iban} {reference}"
    }
}

fn main() {
    console.println(label(Payment.Card("4242")))
}
```

## Optional values, results and unions

- `T?` is `Option[T]`: `some(value)` or `none`.
- `T ! E` is `Result[T, E]`: `ok(value)` or `err(error)`; `!E` alone has no
  success value. See `tondo guide errors`.
- `A | B` is a union of types; `match` on the member's type with `Int(n)`.

```tondo
import std.console

fn describe(value: Int | String): String {
    match value {
        Int(n) => "the number {n}"
        String(s) => "the text {s}"
    }
}

fn main() {
    console.println(describe(4))
    console.println(describe("four"))
}
```

## Identity with `Ref[T]`

Values have no identity: two equal records are the same, and copying one
copies it. `Ref(value)` gives a value an identity that its copies share.
`==` and hashing compare identities, so a `Ref` is a map key whatever it
holds, and `reference.value` reads the value, which never changes. What
changes goes in a `sync.Mutex` (see `tondo guide tasks`). Values that refer
to each other in a cycle, such as two people in each other's list of
friends, are freed like any other once nothing uses them.

```tondo
import std.console

type Module = { name: String, imports: Array[Ref[Module]] }

fn main() {
    let util = Ref(Module { name: "util", imports: [] })
    let http = Ref(Module { name: "http", imports: [util] })
    let json = Ref(Module { name: "json", imports: [util] })
    // `util` is one module with two importers, not two equal copies.
    console.println("{http.value.imports[0] == json.value.imports[0]}")
    let other = Ref(Module { name: "util", imports: [] })
    console.println("{util == other} {util.value.name == other.value.name}")
}
```

## Generics and traits

```tondo
import std.console

trait Shape {
    fn area(self): Float

    /// A default method.
    fn describe(self): String {
        "a shape of area {self.area()}"
    }
}

type Square = { side: Float }

impl Shape for Square {
    fn area(self): Float {
        self.side * self.side
    }
}

/// `Display` makes a type usable in `"{value}"`.
impl Display for Square {
    fn display(self): String {
        "square {self.side}"
    }
}

type Box[T] = { value: T }

fn largest[T: Ordered](items: Array[T]): T? {
    var best: T? = none
    for item in items {
        best = match best {
            some(current) if current.compare(item) >= 0 => some(current)
            _ => some(item)
        }
    }
    best
}

fn total[S: Shape](shapes: Array[S]): Float {
    var sum = 0.0
    for shape in shapes {
        sum += shape.area()                     // a bound's methods by dot
    }
    sum
}

fn main() {
    let square = Square { side: 2.0 }
    console.println(Shape.describe(square))     // on a concrete value: Trait.method
    console.println("{square} {total([square, square])} {largest([3, 9, 4])}")
    let boxed = Box { value: "x" }
    console.println(boxed.value)
}
```

The prelude's traits: `Display`, `Ordered` (`compare`), `Numeric` (`zero`,
`plus`), `Iterator[T]` (`next`). `Copy`, `Equatable`, `Key`, `Send` and
`Share` are capabilities the compiler works out: a type has them when its
parts do.
