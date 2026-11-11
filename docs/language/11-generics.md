# Generics and traits

Generics let one piece of code work for many types, and traits say what
those types must be able to do. Both are resolved when the program compiles:
there are no objects that carry their methods around, and a call through a
trait costs what a direct call costs.

- [Generic functions](#generic-functions)
- [Generic types](#generic-types)
- [Traits](#traits)
- [Implementing a trait](#implementing-a-trait)
- [Bounds](#bounds)
- [Calling a trait's method](#calling-a-traits-method)
- [Traits that require others](#traits-that-require-others)
- [Functions of the trait itself](#functions-of-the-trait-itself)
- [The prelude's traits](#the-preludes-traits)
- [What a type can do by nature](#what-a-type-can-do-by-nature)
- [Hiding a result's type: impl](#hiding-a-results-type-impl)
- [Many types in one collection](#many-types-in-one-collection)
- [Who may implement what](#who-may-implement-what)
- [What traits do not have](#what-traits-do-not-have)

## Generic functions

A type parameter goes in brackets after the function's name:

```tondo
import std.console

fn firstOr[T](values: Array[T], fallback: T): T {
    values.get(0).unwrapOr(fallback)
}

fn pairs[A, B](keys: Array[A], values: Array[B]): Array[(A, B)] {
    keys.zip(values)
}

fn main() {
    console.println("{firstOr([3, 4], 0)} {firstOr([], "none")}")
    console.println("{pairs(["a", "b"], [1, 2])}")
    let empty = firstOr[Float]([], 1.5)        // written, when nothing gives it
    console.println("{empty}")
}
```

```text output
3 none
[(a, 1), (b, 2)]
1.5
```

The type arguments are worked out from the call's arguments and from what
is expected of its result. When that leaves no answer or more than one, they
are written, `firstOr[Float](...)`. Brackets are for types everywhere, in
`Array[Int]` as in `parse[User](text)`; angle brackets are only comparisons.

A generic function is compiled once for each set of types it is used with,
so `firstOr` on integers is as fast as a function written for integers.

## Generic types

Records and enums take type parameters the same way, and their methods name
them:

```tondo
import std.console

type Stack[T] = { items: Array[T] }

fn Stack[T](): Stack[T] {
    Stack { items: [] }
}

fn Stack[T].push(mut self, item: T) {
    self.items.push(item)
}

fn Stack[T].pop(mut self): T? {
    self.items.pop()
}

fn Stack[T].map[U](self, transform: fn(T): U): Stack[U] {
    Stack { items: self.items.map(transform) }
}

enum Tree[T] {
    Leaf
    Node(Tree[T], T, Tree[T])
}

fn Tree[T].size(self): Int {
    match self {
        Tree.Leaf => 0
        Tree.Node(left, _, right) => left.size() + 1 + right.size()
    }
}

fn main() {
    var stack = Stack[Int]()
    stack.push(1)
    stack.push(2)
    let labels = stack.map((n) { "#{n}" })
    console.println("{stack.pop()} {stack.items} {labels.items}")

    let leaf: Tree[Int] = Tree.Leaf
    let tree = Tree.Node(Tree.Node(leaf, 1, leaf), 2, leaf)
    console.println("{tree.size()}")
}
```

```text output
some(2) [1] [#1, #2]
2
```

Type parameters do not vary: an `Array[Int]` is never an `Array[Int |
String]`, even though an `Int` can become that union. Turning one into the
other is a `map`, so that what it costs can be seen.

## Traits

A trait is a set of methods that a type can have. It declares behaviour and
holds no data.

```tondo
import std.console

trait Shape {
    /// Every shape gives its area.
    fn area(self): Float

    /// A method with a body is a default, which a type may replace.
    fn describe(self): String {
        "a shape of area {self.area():.2}"
    }
}

type Circle = { radius: Float }
type Rectangle = { width: Float, height: Float }

impl Shape for Circle {
    fn area(self): Float {
        3.14159 * self.radius * self.radius
    }
}

impl Shape for Rectangle {
    fn area(self): Float {
        self.width * self.height
    }

    fn describe(self): String {
        "a {self.width} by {self.height} rectangle"
    }
}

fn main() {
    let circle = Circle { radius: 1.0 }
    let rectangle = Rectangle { width: 2.0, height: 3.0 }
    console.println(circle.describe())
    console.println(rectangle.describe())
}
```

```text output
a shape of area 3.14
a 2.0 by 3.0 rectangle
```

Inside a trait, `Self` is the type that implements it:
`fn compare(self, other: Self): Int`.

## Implementing a trait

A type has a trait because an `impl` says so. Having methods with the right
names is not enough, which means a type never satisfies a trait by accident,
and reading an `impl` tells exactly which contract it fulfils.

An `impl` writes each method the trait requires, may replace the defaults,
and adds nothing else: a method that is only the type's own is declared
apart, as `fn Circle.diameter(self)`.

An `impl` for a generic type declares its parameters after `impl`:

```tondo
import std.console

type Box[T] = { value: T }

impl[T: Display] Display for Box[T] {
    fn display(self): String {
        "Box({self.value})"
    }
}

fn main() {
    console.println("{Box { value: 42 }} {Box { value: "text" }} {Box { value: Box { value: 1.5 } }}")
}
```

```text output
Box(42) Box(text) Box(Box(1.5))
```

A `Box[T]` can be shown when its `T` can, which the bound says.

## Bounds

A bound, after `:`, says what a type parameter must be able to do. Inside
the function, the methods of its bounds are there:

```tondo
import std.console

trait Shape {
    fn area(self): Float
    fn name(self): String
}

type Square = { side: Float }

impl Shape for Square {
    fn area(self): Float {
        self.side * self.side
    }

    fn name(self): String {
        "square"
    }
}

fn totalArea[S: Shape](shapes: Array[S]): Float {
    var sum = 0.0
    for shape in shapes {
        sum += shape.area()
    }
    sum
}

fn largest[T: Ordered](items: Array[T]): T? {
    var best: T? = none
    for item in items {
        best = match best {
            some(current) if current >= item => some(current)
            _ => some(item)
        }
    }
    best
}

fn describe[T: Shape + Display](shape: T): String {
    "{shape} is a {shape.name()}"
}

impl Display for Square {
    fn display(self): String {
        "[{self.side}]"
    }
}

fn main() {
    let squares = [Square { side: 1.0 }, Square { side: 2.0 }]
    console.println("{totalArea(squares)} {largest([3, 9, 4])} {largest(["pear", "fig"])}")
    console.println(describe(squares[1]))
}
```

```text output
5.0 some(9) some(pear)
[2.0] is a square
```

`+` joins several bounds. Without a bound, a `T` can be stored, passed,
returned and copied, and nothing else, since nothing is known of it.

## Calling a trait's method

- In generic code, on a value whose type has the bound: `shape.area()`.
- On a value of a known type, with a dot too, when one trait of the type has
  a method of that name. The type's own methods go first.
- Through the trait, always: `Shape.area(circle)`. That is the form to use
  when two traits give a type a method of the same name.

```tondo
import std.console

trait Named {
    fn name(self): String
}

type User = { id: Int }

impl Named for User {
    fn name(self): String {
        "user {self.id}"
    }
}

impl Named for Int {
    fn name(self): String {
        "number {self}"
    }
}

fn main() {
    let user = User { id: 1 }
    console.println("{user.name()} | {Named.name(user)} | {7.name()}")
}
```

```text output
user 1 | user 1 | number 7
```

A trait can be implemented for a type of the language, as `Int` above,
within the rule of [who may implement what](#who-may-implement-what).

## Traits that require others

A trait can require another, written as a bound. Implementing it then needs
the other's `impl` too, and its defaults may use the other's methods:

```tondo
import std.console

trait Entity: Display {
    fn id(self): Int

    fn label(self): String {
        "#{self.id()} {self}"
    }
}

type Product = { id: Int, title: String }

impl Display for Product {
    fn display(self): String {
        self.title
    }
}

impl Entity for Product {
    fn id(self): Int {
        self.id
    }
}

fn listing[E: Entity](entities: Array[E]): String {
    entities.map((entity) { entity.label() }).join(", ")
}

fn main() {
    console.println(listing([Product { id: 1, title: "pen" }, Product { id: 2, title: "ink" }]))
}
```

```text output
#1 pen, #2 ink
```

A bound `E: Entity` gives `Display` as well. This is a requirement and not
inheritance: `Entity` does not receive `Display`'s methods or replace them,
and each `impl` writes its own trait's.

## Functions of the trait itself

A trait can declare a function with no `self`, which each type implements.
It is called through the trait, with the type in brackets:

```tondo
import std.console

trait Default {
    fn default(): Self
}

impl Default for Int {
    fn default(): Int {
        0
    }
}

impl Default for String {
    fn default(): String {
        "(empty)"
    }
}

fn orDefault[T: Default](value: T?): T {
    match value {
        some(present) => present
        none => Default.default[T]()
    }
}

fn main() {
    let missing: Int? = none
    let absent: String? = none
    console.println("{orDefault(missing)} {orDefault(absent)} {orDefault(some(5))}")
}
```

```text output
0 (empty) 5
```

This is how a trait says "a value of this type can be made from nothing, or
from bytes, or from an integer": `Number.fromInt[T](3)`,
`serialization.Decode`.

## The prelude's traits

These need no import:

| Trait | A type that implements it | Gives |
| --- | --- | --- |
| `Display` | writes `display(self): String` | `"{value}"`, `join`, being printed |
| `Ordered` | writes `compare(self, other: Self): Int` | `<` `<=` `>` `>=`, `sorted()`, `min()`, `max()` |
| `Iterator[T]` | writes `next(mut self): T?` | `for`, and every adapter |
| `Additive` | writes `zero`, `plus`, `minus` | `+`, `-`, `sum()` |
| `Number` | adds `one`, `fromInt`, `times`, `dividedBy` | `*`, `/`, integer literals |

```tondo
import std.console

pub type Version = { major: Int, minor: Int, patch: Int }

impl Ordered for Version {
    fn compare(self, other: Version): Int {
        if self.major != other.major {
            return self.major.compare(other.major)
        }
        if self.minor != other.minor {
            return self.minor.compare(other.minor)
        }
        self.patch.compare(other.patch)
    }
}

impl Display for Version {
    fn display(self): String {
        "{self.major}.{self.minor}.{self.patch}"
    }
}

fn main() {
    let versions = [
        Version { major: 1, minor: 10, patch: 0 },
        Version { major: 1, minor: 2, patch: 5 },
        Version { major: 0, minor: 9, patch: 9 },
    ]
    console.println("{versions.sorted()} {versions.max()}")
    console.println("{versions[1] < versions[0]} {versions.sorted().map((v) { "{v}" }).join(" < ")}")
}
```

```text output
[0.9.9, 1.2.5, 1.10.0] some(1.10.0)
true 0.9.9 < 1.2.5 < 1.10.0
```

`compare` gives a negative number, zero or a positive one as `self` goes
before, with or after `other`. Every number, `Char`, `String` and `Bool`
have it.

## What a type can do by nature

Some abilities are not implemented: a type has them when its parts do, and
the compiler works it out.

| Ability | Means | Who has it |
| --- | --- | --- |
| `Copy` | assigning it leaves the original usable | nearly everything: numbers, text, records, enums and collections of them |
| `Equatable` | `==` and `!=` | values whose parts can be compared; not functions |
| `Key` | it can be a map's key or a set's element | what can be compared and hashed; not floats |
| `Send` | it can go to another task | values that own what they hold |
| `Share` | several tasks can read it at once | values, and the types of `std.sync` |

They are bounds like any other: `fn unique[T: Key](values: Array[T])` takes
only what can be put in a set.

A type parameter of a function is `Copy` unless it says otherwise, since
nearly every type is, and code that copies its values is the usual kind.
`T: ~Copy` takes that away, for the rare function that also accepts values
that cannot be copied, as a handle to something outside the program
([Ownership](12-ownership.md)).

## Hiding a result's type: impl

A function can return "something that implements this" without naming it.
The caller uses the bound and nothing else:

```tondo
import std.console

type Cursor[T] = { values: Array[T], index: Int }

impl[T] Iterator[(Int, T)] for Cursor[T] {
    fn next(mut self): (Int, T)? {
        let value = self.values.get(self.index)?
        self.index += 1
        some((self.index - 1, value))
    }
}

/// An iterator of the elements with their positions: which type it is
/// stays this module's business.
pub fn indexed[T](values: Array[T]): impl Iterator[(Int, T)] {
    Cursor { values, index: 0 }
}

/// A closure that keeps a count. Its type cannot be written at all.
pub fn counter(start: Int): impl CallMut[fn(): Int] {
    var count = start
    (): Int {
        count += 1
        count
    }
}

fn main() {
    for (index, word) in indexed(["a", "b"]) {
        console.println("{index}: {word}")
    }
    var next = counter(10)
    console.println("{next()} {next()}")
}
```

```text output
0: a
1: b
11 12
```

`impl Bound` is still one concrete type, chosen by the function and known to
the compiler: nothing is boxed and no call is indirect. It lets a module
change what it returns without breaking who calls it, and it is the only
way to return a closure that has captured something, short of the uniform
`fn(...)` type. It goes in a function's result and nowhere else, and every
path of the function returns the same type.

## Many types in one collection

A trait is not a type. `Array[Shape]` does not exist: an array holds values
of one type, and a trait is a property of many. When a collection must hold
different things, say which, in one of three ways.

```tondo
import std.console

type Circle = { radius: Float }
type Square = { side: Float }

/// 1. An enum: the cases are closed and each has a name.
enum Shape {
    Round(Circle)
    Boxy(Square)
}

fn area(shape: Shape): Float {
    match shape {
        Shape.Round(circle) => 3.14159 * circle.radius * circle.radius
        Shape.Boxy(square) => square.side * square.side
    }
}

/// 2. A union: the same, for a few existing types.
fn describe(shape: Circle | Square): String {
    match shape {
        Circle(circle) => "a circle of radius {circle.radius}"
        Square(square) => "a square of side {square.side}"
    }
}

/// 3. Functions as values: when what varies is behaviour, and the cases are open.
type Renderer = { name: String, draw: fn(): String }

fn main() {
    let shapes = [Shape.Round(Circle { radius: 1.0 }), Shape.Boxy(Square { side: 2.0 })]
    console.println("{shapes.map(area)}")

    let mixed: Array[Circle | Square] = [Circle { radius: 1.0 }, Square { side: 2.0 }]
    console.println("{mixed.map(describe)}")

    let side = 3.0
    let renderers = [
        Renderer { name: "dot", draw: () { "." } },
        Renderer { name: "box", draw: () { "[{side}]" } },
    ]
    console.println("{renderers.map((renderer) { "{renderer.name}: {renderer.draw()}" })}")
}
```

```text output
[3.14159, 4.0]
[a circle of radius 1.0, a square of side 2.0]
[dot: ., box: [3.0]]
```

This is the trade Tondo makes. There are no trait objects, so there are no
tables of methods looked up as the program runs, no question of how long a
borrowed object lives, and no casts from a general type back to a
particular one. In return, a collection of different things says what they
are.

## Who may implement what

An `impl Trait for Type` goes in the module that declares the trait, or in
the one that declares the type. A third module cannot give someone else's
type someone else's trait.

That one rule means a pair of trait and type has at most one `impl` in a
whole program, wherever its packages come from, so adding a dependency
never changes what a method call means. To give a foreign type a foreign
trait, wrap it in a newtype of your own, which is yours to implement for.

## What traits do not have

- **Associated types and constants.** A trait's parameters are in its
  brackets, as `Iterator[T]`.
- **Trait objects**, as above.
- **Inherited implementations.** A trait that requires another does not
  receive or replace its methods.
- **Operators**, except arithmetic through `Additive` and `Number`.
- **Automatic conversions.** Nothing like `From` and `Into` runs unseen: a
  conversion is a function someone calls.
- **Generics over values or over type constructors.** An array's length is
  not part of its type.

Each of these keeps a method call resolvable by reading the code around it.

Next: [Ownership](12-ownership.md).
