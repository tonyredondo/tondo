# Pitfalls

Mistakes that come from other languages, the error each gives, and the fix.
Every error message says what to write; `tondo check --json` has it as data.

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

A trait's method on a concrete value is called through the trait:
`Shape.area(square)`; `value.area()` works on a generic `T: Shape`.

```tondo error E1001
trait Shape {
    fn area(self): Int
}

type Square = { side: Int }

impl Shape for Square {
    fn area(self): Int {
        self.side * self.side
    }
}

fn main() {
    let square = Square { side: 2 }
    let a = square.area()
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

```tondo error E1002
type User = { name: String }

fn User.name(self): String {
    self.name
}

fn main() {}
```

Numbers do not convert by themselves: `Int(3.5)?`, `Float(n)`; a `Byte`
literal is `Byte(255u8)`.

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
