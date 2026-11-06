# Pitfalls

Mistakes that come from other languages, the error each gives, and the fix.
Every error message says what to write; `tondo check --json` has it as data.

| Elsewhere | Tondo |
|---|---|
| `let mut x = 0` | `var x = 0` |
| `while c { }`, `loop { }` | `for c { }`, `for { }` |
| `fn f() -> Int`, `fn(Int) -> Int` | `fn f(): Int`, `fn(Int): Int` |
| `fn id<T>(x: T)`, `Vec<T>`, `[T]` | `fn id[T](x: T)`, `Array[T]` |
| `HashMap<K, V>`, `{}` | `Map[K, V]`, `[:]` |
| `null`, `nil`, `None`, `Some(x)` | `none`, `some(x)` |
| `x as Int` | `Int(x)` |
| `\|x\| x * 2`, `x => x * 2` | `(x) { x * 2 }` |
| `struct P { }` and `impl P { fn m() }` | `type P = { }` and `fn P.m(self)` |
| `try`, `catch`, `throw` | `T ! E`, `?`, `match`, `fail error` |
| `defer func() { recover() }()` | `recover { ... }`, a `T ! Panic` |
| `async`, `await` | plain calls; `spawn f()` and `.wait()` |
| `go f()`, a promise nobody awaits | `spawn detached f()`; a plain `spawn` is waited at the end of its block |
| `switch` | `match` |
| `&&`, `\|\|`, `!x` | `and`, `or`, `not x` |
| `len(x)`, `x.len()`, `x.length` | `x.length()` |
| `` `${x}` ``, `"{}".format(x)` | `"{x}"` |
| a method guessed from another library | `tondo api String`, `tondo api std.http.Client`: its methods and an example |

No semicolons; a newline ends a statement.

```tondo error E0002
fn main() {
    let x = 1;
}
```

`+` does not join strings: interpolate, `"Hello, {name}"`, or `a.concat(b)`.

```tondo error E1102
import std.console

fn main() {
    let name = "Ada"
    console.println("Hello, " + name)
}
```

`{...}` in a string interpolates. Write `{{` and `}}` for braces, or a raw
string `r"..."`; web routes use `:id`, not `{id}`.

```tondo error E1001
fn main() {
    let route = "/users/{userId}"
}
```

No shadowing: a new binding needs a new name, also in inner blocks, closures
and patterns, and it cannot reuse the name of a function of the module.

```tondo error E1003
fn main() {
    let total = 1
    if total > 0 {
        let total = 2
    }
}
```

No `if let`: match the value.

```tondo error E0004
fn main() {
    let x: Int? = none
    if let some(v) = x {
    }
}
```

No `|` in patterns: one arm per pattern, or `_` with a guard.

```tondo error E0004
fn size(n: Int): String {
    match n {
        1 | 2 => "small"
        _ => "big"
    }
}

fn main() {}
```

`some`, `none`, `ok`, `err` are lowercase; there is no `null`.

```tondo error E1001
fn main() {
    let x = Some(1)
}
```

A match covers every case; the error names the one missing.

```tondo error E1204
enum Color {
    Red
    Green
}

fn name(color: Color): String {
    match color {
        Color.Red => "red"
    }
}

fn main() {}
```

A trait's method is called with a dot, as `square.area()`. When two traits of a
value give the same method, the call names one: `Shape.area(square)`.

```tondo error E1004
trait Shape {
    fn area(self): Int
}

trait Land {
    fn area(self): Int
}

type Square = { side: Int }

impl Shape for Square {
    fn area(self): Int {
        self.side * self.side
    }
}

impl Land for Square {
    fn area(self): Int {
        0
    }
}

fn main() {
    let square = Square { side: 2 }
    _ = square.area()
}
```

What changes a collection needs `var`: `var items = [1, 2]`.

```tondo error E1411
fn main() {
    let items = [1, 2]
    items.push(3)
}
```

A fallible call's result must be used: `?`, `match`, or `_ = call()`.

```tondo error E1303
enum E {
    Bad
}

fn risky(): Int ! E {
    fail E.Bad
}

fn main() {
    risky()
}
```

A closure that fails declares its result when nothing else gives it:
`(): Int ! LoadError { ... }`.

```tondo error E1302
enum LoadError {
    Missing
}

fn main() {
    let load = () {
        fail LoadError.Missing
    }
}
```

`else` goes on the line of the `}` before it: `} else {`.

```tondo error E0004
fn main() {
    let ready = true
    if ready {
    }
    else {
    }
}
```

There is no C-style `for`: `for i in 0..count`, `for condition`, `for`.

```tondo error E0004
fn main() {
    for (i = 0; i < 3; i++) {
    }
}
```

Keywords cannot be names: `match`, `test`, `suite`, `with`, `as`, `derive`,
`fail`, `select`, `spawn`, `defer`, `in`, `not`, `and`, `or`.

```tondo error E0004
fn main() {
    let match = 1
}
```

A method cannot share its name with a field: `fn User.displayName(self)`.

```tondo error E1505
type User = { name: String }

fn User.name(self): String {
    self.name
}

fn main() {}
```

Numbers do not convert by themselves: `Int(3.5)?`, `Float(n)`, `Byte(n)?`;
only a literal takes the type it is expected to have, a `Byte` too.

```tondo error E1102
fn main() {
    let x: Int = 3.5
}
```

A default value is a constant; for a computed one, take an optional
parameter and decide inside: `limit: Int? = none`.

```tondo error E1102
fn now(): Int {
    1
}

fn wait(limit: Int = now()) {}

fn main() {}
```

A public constant states its type: `pub const Max: Int = 10`.

```tondo error E1115
pub const Max = 10

fn main() {}
```

A function of a script does not see the script's bindings: pass them.

```tondo error E1001
import std.console

let name = "Ada"

fn greet() {
    console.println("Hello, {name}")
}

greet()
```

A line that starts with `+`, `*`, `and` or another binary operator
continues the one before, but one that starts with `-` is a negative value
of its own, as a `match` arm's `-1 =>`: a subtraction that goes on to the
next line ends the first with `-`. A negative value that nothing uses warns
(`W1014`).

```tondo
fn total(price: Int, discount: Int, fee: Int): Int {
    price -
        discount
        + fee
}

fn main() {}
```

A query of `std.sql` is a string literal where a `sql.Query` is expected,
whose values are parameters; a `String` built before is only text. Keep a
part of a query as a `sql.Query`: `let wanted: sql.Query = "id > {min}"`.

```tondo error E1102
import std.sql

fn names(db: sql.Database, min: Int): Array[String] ! sql.SqlError {
    let text = "SELECT name FROM users WHERE id > {min}"
    db.query[String](text)?
}

fn main() {}
```

A type can take a name of the prelude that a domain uses too, as `Key`, `Range`,
`Command`, `Group` or `Pipeline`, and in its module it is that type. It cannot
take the language's own vocabulary: `Int`, `String`, `Array`, `Option`, `Copy`,
`Display`, `Failure` and the like. Name it after what it holds.

```tondo error E1005
pub type Copy = { id: String }

fn main() {}
```

A function that gives a text writes nothing: `cli.markup("[red]no[/]")` alone
shows nothing, and `W1018` says so. Print what it gives, with
`console.println(cli.markup("[red]no[/]"))`, or discard it with `_ = ...`.
