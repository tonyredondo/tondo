# Runbook: changing what other code uses

A name that is wrong, a function that needs one more parameter, code that
nothing calls any longer: each is a change to every file that uses the
declaration. A search for its text also finds comments and other names that
hold it. These tools work on what the compiler knows: they find the places
that name that declaration, rewrite them all, and refuse a change that would
leave the program not compiling.

```file tondo.toml
[package]
name = "shop"
```

```file src/main.to
import std.console
import shop.prices

fn main() {
    console.println("{prices.withTax(prices.total([250, 400]))}")
}
```

```file src/prices/prices.to
/// The sum of some prices, in cents.
pub fn total(cents: Array[Int]): Int {
    cents.sum()
}

/// A price with its tax.
pub fn withTax(cents: Int): Int {
    cents + (cents * 21 + 50) / 100
}

/// What the old shop charged for sending an order.
pub fn oldShipping(cents: Int): Int {
    if total([cents]) >= 5000 { 0 } else { 495 }
}
```

```file src/prices/prices_test.to
test addsThePrices {
    assert(total([250, 400]) == 650)
}

test addsTheTax {
    assert(withTax(250) == 303)
}
```

The project is committed, which is what the last steps compare with.

```sh
git init -q
git config user.name Ada
git config user.email ada@example.com
git config commit.gpgsign false
git add .
git commit -q -m "The shop"
```

## Who uses it

Before changing a declaration, see what the change reaches. A place is a
`file`, a `line` and a `column`, from 1: here the name `total` where it is
declared. `references` gives every place that names it, in the tests too.

```mcp
references {"file": "src/prices/prices.to", "line": 2, "column": 8}
=> src/main.to:5:45
=> src/prices/prices_test.to:2:12
```

`calls` gives the functions that call it instead, and with `direction`
`outgoing` those it calls; `definition` and `hover` answer from a place that
uses a name, `symbols` with `query` finds a declaration by its name, and
`implementations` gives the `impl`s of a trait or of a type.

## A new name

`rename` rewrites the declaration and every reference to it.

```mcp
rename {"file": "src/prices/prices.to", "line": 2, "column": 8, "new_name": "sum"}
=> src/main.to: 1 edits
=> src/prices/prices_test.to: 1 edits
check {}
=> it compiles
```

## One more parameter

The tax is not 21 percent everywhere. `refactor` with the kind `signature`
takes the new parameters, and gives every call the value written after `=`,
so that the program does what it did.

```mcp
refactor {"file": "src/prices/prices.to", "line": 7, "column": 8, "kind": "signature", "argument": "cents: Int, percent: Int = 21"}
=> src/main.to: 1 edits
=> src/prices/prices_test.to: 1 edits
```

The calls now read `withTax(250, 21)`. The body is the part that is yours.

```edit src/prices/prices.to
-    cents + (cents * 21 + 50) / 100
+    cents + (cents * percent + 50) / 100
```

```mcp
check {}
=> it compiles
```

## What the change reached

`test` with `affected` and a commit runs only the tests that reach what
changed since it: in a large project, the quick answer before the whole suite.

```mcp
test {"affected": "HEAD"}
=> addsTheTax ... ok
```

## What it breaks for others

For a package that others import, `api_diff` with a commit says what its
public API lost or changed since then, which breaks who uses it, and what it
gained.

```mcp
api_diff {"rev": "HEAD"}
=> "breaks":true
=> shop.prices.total
=> pub fn withTax(cents: Int, percent: Int): Int
```

The rename took `total` away and the parameter changed `withTax`: nothing in a
program, and a version that says so in a library.

## What nothing uses

`unreached` gives the code that neither `main` nor a test reaches, and
`refactor` with `delete` takes a declaration out, refused while something
names it.

```mcp
unreached {}
=> oldShipping
refactor {"file": "src/prices/prices.to", "line": 12, "column": 8, "kind": "delete"}
=> src/prices/prices.to: 1 edits
unreached {}
=> "unreached":[]
test {}
=> 2 passed
```

## The other changes

`refactor` also moves a declaration to another module (`move`), groups
parameters into a record (`group`, with `Point: x, y`), extracts a trait
(`trait`, with `Shape: area, name`), makes a function a method or the reverse
(`to_method`, `to_function`), inlines a call or an alias (`inline_call`,
`inline_alias`), and with `end_line` and `end_column` extracts the selection
as a variable, a function or a constant.

`graph` with a `module` and `reverse` gives the modules that import one. To
find the commit that broke something: `runbook-bisect`.
