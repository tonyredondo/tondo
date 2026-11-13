# Control flow and operators

Tondo has one conditional, one loop and one way to tell cases apart. This
chapter covers `if`, `for` in all its forms, leaving a block early, `defer`,
assignment, and the operators with their precedence. `match` has a chapter
of its own with [enums](06-enums.md).

- [if](#if)
- [for: the one loop](#for-the-one-loop)
- [Looping over a collection in place](#looping-over-a-collection-in-place)
- [break, continue and return](#break-continue-and-return)
- [defer](#defer)
- [Assignment](#assignment)
- [Operators](#operators)
- [in](#in)
- [Precedence](#precedence)

## if

```tondo
import std.console

fn grade(score: Int): String {
    if score >= 90 {
        "excellent"
    } else if score >= 50 {
        "pass"
    } else {
        "fail"
    }
}

fn main() {
    let score = 72
    let label = if score % 2 == 0 { "even" } else { "odd" }
    console.println("{grade(score)}, {label}")
    if score > 100 {
        console.println("impossible")
    }
}
```

```text output
pass, even
```

`if` is an expression. When its value is used it has an `else`, and all its
branches give the same type; without one it is a statement.

A condition is a `Bool` and nothing else. A number, a string, a collection
or an option is never true or false by itself, so the question is written:

| Not Tondo | Tondo |
| --- | --- |
| `if count` | `if count != 0` |
| `if name` | `if not name.isEmpty()` |
| `if items` | `if not items.isEmpty()` |
| `if user` | `if user.isSome()`, or a `match` to get the value |
| `a ? b : c` | `if a { b } else { c }` |

## for: the one loop

There is no `while` and no `loop`. `for` does all three jobs:

```tondo
import std.console

fn main() {
    // Over a range: 0, 1, 2. `0..=3` would include the 3.
    for i in 0..3 {
        console.println("turn {i}")
    }

    // While a condition holds.
    var n = 27
    var steps = 0
    for n != 1 {
        n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 }
        steps += 1
    }
    console.println("{steps} steps")

    // Until a break.
    var power = 1
    for {
        power *= 2
        if power > 1000 {
            break
        }
    }
    console.println("{power}")
}
```

```text output
turn 0
turn 1
turn 2
111 steps
1024
```

`for x in source` goes through anything that gives values one at a time: a
range, an array, a map, a set, the characters of a string, a channel, or any
type that implements `Iterator[T]`. The variable can be a pattern.

```tondo
import std.console

fn main() {
    let names = ["ada", "bo"]
    for name in names {
        console.println(name)
    }
    for (index, name) in names.enumerate() {
        console.println("{index}: {name}")
    }
    let ages = ["ada": 36, "bo": 20]
    for (name, age) in ages {
        console.println("{name} is {age}")
    }
    for letter in "año".chars() {
        console.println("{letter}")
    }
}
```

```text output
ada
bo
0: ada
1: bo
ada is 36
bo is 20
a
ñ
o
```

There is no C-style `for (init; condition; step)`. The start goes before
the loop and the step inside it. A record literal in the header of a `for`,
an `if` or a `match` goes in parentheses, `for n in (Countdown { from: 3 })`,
since its `{` would otherwise open the body.

## Looping over a collection in place

A plain `for item in items` gives each element as a value of its own:
changing `item` would change a copy. To change the elements where they are,
the loop variable says `mut`; to replace them, `var`; and `ref` reads them
without copying, which matters when they are large.

```tondo
import std.console

type Account = { owner: String, balance: Int }

fn main() {
    var accounts = [
        Account { owner: "ada", balance: 10 },
        Account { owner: "bo", balance: 20 },
    ]
    for mut account in accounts {
        account.balance += 5                // changes the element itself
    }
    for ref account in accounts {
        console.println("{account.owner} {account.balance}")
    }

    var scores = ["ada": 1, "bo": 2]
    for (_, mut score) in scores {
        score *= 10                         // a map's values; its keys never change
    }

    var squares = [1, 2, 3, 4]
    for var value in squares {
        value = value * value               // replaces the element
    }
    console.println("{scores} {squares}")
}
```

```text output
ada 15
bo 25
[ada: 10, bo: 20] [1, 4, 9, 16]
```

While such a loop runs, the collection is lent to it: the body cannot add
or remove elements, and with `mut` or `var` cannot touch the collection by
its name at all. That is checked when the program compiles
([Ownership](12-ownership.md)).

## break, continue and return

```tondo
import std.console

fn firstNegative(values: Array[Int]): Int? {
    for value in values {
        if value == 0 {
            continue            // on to the next element
        }
        if value < 0 {
            return value  // out of the function
        }
    }
    none
}

fn main() {
    console.println("{firstNegative([3, 0, -4, -9])} {firstNegative([1, 2])}")
}
```

```text output
some(-4) none
```

- `break` ends the nearest loop and `continue` starts its next turn. Loops
  have no labels and `break` carries no value: to leave two loops at once,
  put them in a function and `return`.
- `return` leaves the nearest function or closure, with a value if it has
  one to give. Inside a closure it ends the closure, never the function
  around it.
- `fail error` leaves a function that can fail, with the error
  ([Errors](07-errors.md)).

## defer

`defer` sets something to run when the block it is in ends, whichever way
that happens: its end, a `return`, a `fail`, a `break`, a panic, or the
cancellation of the task.

```tondo
import std.console

fn work(early: Bool): String {
    console.println("open")
    defer console.println("close")
    defer {
        console.println("flush")
    }
    if early {
        return "left early"
    }
    console.println("working")
    "done"
}

fn main() {
    console.println(work(false))
    console.println(work(true))
}
```

```text output
open
working
flush
close
done
open
flush
close
left early
```

- Several `defer`s run last first, so that what was acquired last is
  released first.
- A deferred call's arguments are evaluated where the `defer` is written,
  and the call is made when the block ends.
- What is deferred cannot fail and gives nothing: an operation that may
  fail is handled inside a `defer { ... }` block.
- `defer` is not a destructor. It puts the cleanup next to what it cleans
  up, and nothing runs that the code does not show.

## Assignment

Assignment is a statement, not an expression: `a = b = c` and
`if (x = next())` do not exist.

```tondo
import std.console

type Point = { x: Int, y: Int }

fn main() {
    var point = Point { x: 1, y: 2 }
    var values = [10, 20, 30]
    var ages = ["ada": 36]

    point.x = 5                     // a field
    values[0] = 11                  // an element
    ages["bo"] = 20                 // an entry: added, or replaced
    values[1] += 1                  // compound: + - * / % & ^ | << >>

    var left = "L"
    var right = "R"
    (left, right) = (right, left)                       // a swap
    (values[0], values[2]) = (values[2], values[0])     // also of elements
    console.println("{point.x} {values} {ages} {left}{right}")
}
```

```text output
5 [30, 21, 11] [ada: 36, bo: 20] RL
```

The left side must be something that can change: a `var`, or a field or
element reached through one. The right side is evaluated in full before
anything is written, so a failure there leaves the target as it was. There
is no `++` or `--`: write `index += 1`.

## Operators

| Operators | On | Notes |
| --- | --- | --- |
| `+` `-` `*` `/` `%` | numbers of the same type | Integer overflow panics. `/` on integers truncates; `%` is not defined for floats |
| `-x` | signed integers and floats | |
| `==` `!=` | two values of one type | Any type whose parts can be compared |
| `<` `<=` `>` `>=` | numbers, `Char`, `String`, and types with `Ordered` | Not chained: `0 < n and n < 10` |
| `and` `or` `not` | `Bool` | `and` and `or` stop as soon as the answer is known |
| `&` `^` `\|` `~` `<<` `>>` | integers and `Byte` | A shift count out of range panics |
| `..` `..=` | integers and `Char` | A range, without and with its end |
| `in` | a value and a collection | Membership, below |
| `?` | a `T ! E` or a `T?` | Gives the value, or leaves with the failure |
| `??` | a `T?` or a `T ! E`, and a fallback | The value, or else the fallback, evaluated only then |
| `?.` | a `T?` and what follows it | Goes on only when there is a value ([Errors](07-errors.md#a-fallback-and-a-chain--and-)) |
| `with` | a record | A copy with some fields changed |
| `\|` | two commands | A pipeline of processes |

```tondo
import std.console

fn main() {
    let n = 7
    console.println("{7 / 2} {7.0 / 2.0} {-7 % 3} {2 * 3 + 4} {2 * (3 + 4)}")
    console.println("{0 < n and n < 10} {not (n == 7) or n % 2 == 1}")
    console.println("{n << 2} {n & 3} {n | 8} {n ^ 1} {~n}")
}
```

```text output
3 3.5 -1 10 14
true true
28 3 15 6 -8
```

Operators mean what the language says and nothing else. A type of the
program takes `+` and `-` by implementing `Additive`, and `*` and `/` with
`Number` ([Numbers](10-numbers.md)); arrays of numbers take the arithmetic
operators element by element ([Collections](08-collections.md)). No other
operator can be given to a type, so reading `a + b` never requires finding
out who defined it.

Some operators other languages have are named instead:

| Elsewhere | Tondo |
| --- | --- |
| `"a" + "b"` | `"{a}{b}"`, or `a.concat(b)` |
| `[1] + [2]` | `first.concat(second)` |
| `!x`, `&&`, `\|\|` | `not x`, `and`, `or` |
| `x++` | `x += 1` |
| `a ?: b` | `a ?? b` |
| `c ? a : b` | `if c { a } else { b }` |
| `0 < x < 10` | `0 < x and x < 10` |

## in

`in` asks whether a value is in a collection, and gives a `Bool`:

```tondo
import std.console

fn main() {
    let primes = [2, 3, 5, 7]
    let ages = ["ada": 36]
    let vowels = Set['a', 'e', 'i', 'o', 'u']
    console.println("{5 in primes} {"ada" in ages} {'e' in vowels}")
    console.println("{'l' in "hello"} {5 in 0..10} {10 in 0..10} {10 in 0..=10}")
}
```

```text output
true true true
true true false true
```

In a map it looks at the keys, and in a string at one character: a piece of
text is found with `text.contains("ell")`. In a `for`, an `in` at the top
means iteration, so a loop that repeats while something is a member wraps
its condition: `for (item in pending) { ... }`.

## Precedence

From the tightest to the loosest:

| | Operators | Groups |
| ---: | --- | --- |
| 1 | calls, `[...]`, `.`, `?` | left to right |
| 2 | `-x`, `not`, `~`, `spawn` | prefix |
| 3 | `*` `/` `%` | left to right |
| 4 | `+` `-` | left to right |
| 5 | `<<` `>>` | left to right |
| 6 | `&` | left to right |
| 7 | `^` | left to right |
| 8 | `\|` | left to right |
| 9 | `..` `..=` | no chaining |
| 10 | `<` `<=` `>` `>=` `in` | no chaining |
| 11 | `==` `!=` | no chaining |
| 12 | `and` | left to right |
| 13 | `or` | left to right |
| 14 | `with` | left to right |
| 15 | `??` | right to left |

`not` binds tighter than a comparison, so `not a == b` is `(not a) == b`:
write `a != b`, or `not (a == b)`. `??` binds last of all, so everything to
its right is the fallback: `count ?? 0 + 1` is `count ?? (0 + 1)`, and the
other reading takes parentheses, `(count ?? 0) + 1`. `tondo fmt` keeps only the parentheses
the table makes necessary.

Next: [Records, tuples and newtypes](05-records.md).
