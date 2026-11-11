# Records, tuples and newtypes

A program's data is made of a few shapes. This chapter covers the ones that
hold several values at once, and the one that gives a value a name of its
own: records, tuples and newtypes. Enums, which hold one of several
alternatives, are the [next chapter](06-enums.md).

- [Records](#records)
- [Defaults](#defaults)
- [A copy with changes: with](#a-copy-with-changes-with)
- [Methods and constructors](#methods-and-constructors)
- [Private fields](#private-fields)
- [Generic records](#generic-records)
- [Tuples](#tuples)
- [Newtypes](#newtypes)
- [Annotations on fields](#annotations-on-fields)
- [What a record is not](#what-a-record-is-not)

## Records

A record has fields with names. It is declared with `type`, and built with a
literal that names every field:

```tondo
import std.console

pub type User = {
    id: Int
    name: String
    email: String?
}

fn main() {
    let ada = User { id: 1, name: "Ada", email: none }

    // A binding with the field's name needs no `name: name`.
    let id = 2
    let name = "Grace"
    let grace = User { id, name, email: "grace@example.com" }

    console.println("{ada.name} {ada.email} {grace.id} {grace.email}")
}
```

```text output
Ada none 2 some(grace@example.com)
```

Fields are separated by new lines or commas. A record is a value: assigning
it copies it, two records with equal fields are equal, and a `let` record
cannot be changed, field by field or otherwise. Through a `var` its fields
are assigned as any variable is, `user.name = "Ada L."`.

A record's type is its name. Two records with the same fields are different
types, and one is never used as the other.

## Defaults

A field may have a default, which a literal that leaves the field out takes:

```tondo
import std.console
import std.json

pub type Server = {
    host: String
    port: Int = 8080
    tags: Array[String] = []
    proxy: String? = none
}

fn main(): !json.JsonError {
    let plain = Server { host: "localhost" }
    let custom = Server { host: "example.com", port: 443, tags: ["web"] }
    console.println("{plain.port} {plain.tags} {custom.port} {custom.tags}")

    // Reading a record gives a missing field its default too.
    let read = json.decode[Server]("""
        {{ "host": "db", "tags": ["sql"] }}
        """)?
    console.println("{read.host} {read.port} {read.tags}")
}
```

```text output
8080 [] 443 [web]
db 8080 [sql]
```

A default is a constant, as a parameter's is: a literal, `none`, `[]`,
`[:]`, a constant of the module, or a variant or record made of those. A
record with a default for every setting is the way to give a function many
options: the call names only what differs.

## A copy with changes: with

`with` makes a new record from another, changing the fields it names. The
original is as it was.

```tondo
import std.console

type Order = { id: Int, status: String, items: Array[String], note: String = "" }

fn main() {
    let placed = Order { id: 7, status: "placed", items: ["tea"] }
    let paid = placed with { status: "paid" }
    let status = "shipped"
    let shipped = paid with { status, note: "by courier" }      // `status: status`
    console.println("{placed.status} {paid.status} {shipped.status} {shipped.note} {shipped.items}")
}
```

```text output
placed paid shipped by courier [tea]
```

This is how state is moved forward without changing anything in place:
`fn apply(order: Order, event: Event): Order` returns `order with { ... }`.
In code that keeps one mutable value, the result is assigned back,
`order = order with { status: "paid" }`, or the field is simply assigned.

## Methods and constructors

Methods are functions named with the type, declared outside it
([Functions](03-functions.md) has the rules):

```tondo
import std.console

pub type Rectangle = { width: Float, height: Float }

/// The constructor: a function with the type's name.
pub fn Rectangle(side: Float): Rectangle {
    Rectangle { width: side, height: side }
}

pub fn Rectangle.area(self): Float {
    self.width * self.height
}

pub fn Rectangle.scale(mut self, factor: Float) {
    self.width *= factor
    self.height *= factor
}

/// `Display` is what `"{value}"` writes.
impl Display for Rectangle {
    fn display(self): String {
        "{self.width}x{self.height}"
    }
}

fn main() {
    var square = Rectangle(2.0)
    square.scale(1.5)
    console.println("{square} has area {square.area()}")
}
```

```text output
3.0x3.0 has area 9.0
```

A record cannot be interpolated until it has a `Display`: the compiler does
not guess how a `User` should look to a person. For looking at one while
programming, `tondo debug` and the dumps show every field, and
`json.encode(value)` writes any record.

## Private fields

Everything in a module is private to it unless it says `pub`. In a `pub`
record the fields are public too, except those marked `priv`:

```tondo
pub type Account = {
    owner: String
    priv balance: Int
}

pub fn open(owner: String): Account {
    Account { owner, balance: 0 }
}

pub fn Account.deposit(mut self, amount: Int) {
    if amount > 0 {
        self.balance += amount
    }
}

pub fn Account.available(self): Int {
    self.balance
}

fn main() {}
```

Another module can read `account.owner`, call `account.available()`, and
nothing else: it cannot read `balance`, set it, or build an `Account` with a
literal, since it could not give the private field a value. That is how a
type keeps a rule about its own data, here that a balance only grows by
deposits. From outside the module:

```text
error[E1502]: `accounts.Account` has private fields, so only its module can build it with a literal; call a function of that module
error[E1501]: field `balance` of `accounts.Account` is private to its module
```

- A record whose private fields all have defaults can still be built
  outside, by a literal that leaves them out.
- A private field still counts in `==`, in hashing and in copying: `priv`
  hides how a value is made, not what it is.
- `priv` in a type that is not `pub` is an error, since the type is already
  seen only by its module.
- A method cannot have the name of a field, so `value.name` is never
  ambiguous:

```text
error[E1505]: `balance` is a field of `accounts.Account`; a method with a receiver needs another name
```

## Generic records

A record can leave types open:

```tondo
import std.console

type Page[T] = {
    items: Array[T]
    number: Int
    total: Int
}

fn Page[T].isLast(self): Bool {
    self.number * 10 >= self.total
}

fn firstPage[T](items: Array[T]): Page[T] {
    Page { items: items.take(10), number: 1, total: items.length() }
}

fn main() {
    let names = firstPage(["ada", "bo", "cy"])
    let numbers = firstPage((1..=25).collect())
    console.println("{names.items} {names.isLast()} {numbers.items.length()} {numbers.isLast()}")
}
```

```text output
[ada, bo, cy] true 10 false
```

The type argument is worked out from the literal's fields, or written:
`Page[String] { ... }`. [Generics and traits](11-generics.md) covers bounds.

## Tuples

A tuple holds a fixed number of values with no names. It is for values that
travel together briefly: two results of one function, a map's entry, a swap.

```tondo
import std.console

fn divide(dividend: Int, divisor: Int): (Int, Int) {
    (dividend / divisor, dividend % divisor)
}

fn main() {
    let result = divide(17, 5)
    console.println("{result.0} remainder {result.1}")

    let (quotient, remainder) = divide(100, 7)
    console.println("{quotient} remainder {remainder}")

    let entries: Array[(String, Int)] = [("ada", 36), ("bo", 20)]
    for (name, age) in entries {
        console.println("{name}: {age}")
    }
    console.println("{[1, 2, 3].zip(["a", "b", "c"])}")
}
```

```text output
3 remainder 2
14 remainder 2
ada: 36
bo: 20
[(1, a), (2, b), (3, c)]
```

Two tuples have the same type when their elements do, position by position.
`()` is the empty tuple, the value of `Unit`, and there is no tuple of one:
`(value)` is `value` in parentheses.

When the positions mean something that will last, and above all in a public
API, a record is better: `point.x` says more than `point.0`, and a field can
be added to a record without breaking who uses it.

## Newtypes

`type Name = Other` makes a new type that holds a value of another. It costs
nothing when the program runs and stops one kind of value from being passed
as another:

```tondo
import std.console

pub type Cents = Int
pub type Quantity = Int

fn total(price: Cents, quantity: Quantity): Cents {
    Cents(price.value * quantity.value)
}

/// A newtype starts with no methods: these are the ones it is given.
fn Cents.format(self): String {
    "{self.value / 100}.{self.value % 100:02}"
}

fn main() {
    let price = Cents(1999)
    let quantity = Quantity(3)
    console.println(total(price, quantity).format())
}
```

```text output
59.97
```

`total(quantity, price)` does not compile, and neither does `total(1999,
3)`. A newtype is built by calling it and opened with `.value`.

A newtype can be copied, compared, used as a key and sent to a task when
its value can, and has none of its value's methods or operators: `Cents`
has no `+` until the program implements `Additive` for it
([Numbers](10-numbers.md)). To make another name that is the same type, with
everything it has, use `alias` ([Values](02-values.md)).

For a value that must hold a rule, as an email address that was checked,
make it a record with a private field and a function that builds it: a
newtype's `.value` is as public as the type.

## Annotations on fields

An annotation says something about a field to the tools and libraries that
read it. It never changes the field's type or value.

```tondo
import std.console
import std.json

type Account = {
    @name("user_id")
    userId: Int
    /// Left out when it is none, where JSON would write null.
    @omitNone
    nickname: String?
    @ignore
    cached: Int = 0
    @secret
    token: String
}

fn main() {
    let account = Account { userId: 7, nickname: none, cached: 99, token: "s3cret" }
    console.println(json.encode(account))
}
```

```text output
{"user_id":7,"token":"s3cret"}
```

| Annotation | On | Says |
| --- | --- | --- |
| `@name("text")` | a field, a variant | its name outside the program, in every format |
| `@ignore` | a field | it is neither written nor read; it has a default or is optional |
| `@omitNone` | a field, a type | it is left out when it is `none` |
| `@secret` | a field, a type | the debugger and the dumps show `<secret>` for it |
| `@deprecated("use X")` | a function, a method, a type | every use warns, with the text |

`@secret` hides a value from the tools, not from the program: `Display`,
JSON and `==` still see it. A program declares annotations of its own,
`@Check(min: 3)`, and reads them through reflection
([Metaprogramming](17-meta.md)).

## What a record is not

A record has no inheritance, no fields computed behind an access, no
initializer that runs unseen, and no hidden constructor. Building one
evaluates the expressions in its literal, in order, and nothing else. What
several types share is said with a [trait](11-generics.md), and what a type
contains is said by putting one record inside another.

Next: [Enums, unions and match](06-enums.md).
