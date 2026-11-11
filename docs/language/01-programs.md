# Programs, scripts and the shape of a file

A Tondo program is a file whose name ends in `.to`, or a project of them.
This chapter is about the file itself: where a program starts and how it
ends, what a script is, how lines and comments work, and how things are
named.

- [A program starts at main](#a-program-starts-at-main)
- [How a program ends](#how-a-program-ends)
- [A script](#a-script)
- [Lines and statements](#lines-and-statements)
- [The value of a block](#the-value-of-a-block)
- [Comments and documentation](#comments-and-documentation)
- [Names](#names)
- [Keywords](#keywords)
- [The order things are evaluated in](#the-order-things-are-evaluated-in)

## A program starts at main

```tondo
import std.console

fn main() {
    console.println("Hello, Tondo")
}
```

```text output
Hello, Tondo
```

`tondo run hello.to` compiles the file to a native executable and runs it,
and `tondo build hello.to --output hello` leaves the executable. There is no
interpreter, so both run the same code.

`main` takes no parameters and gives no value. What a program is given comes
from the standard library, when it asks: `env.args()` for its arguments,
`env.get("HOME")` for its environment, `console.readLine()` for its input.

```tondo
import std.console
import std.env

fn main() {
    let names = env.args()
    if names.isEmpty() {
        console.println("usage: greet <name>...")
        return
    }
    for name in names {
        console.println("Hello, {name}")
    }
}
```

`tondo run greet.to -- Ada Grace` gives it two arguments: what follows `--`
goes to the program.

A `main` may fail. It can say with what, as any function does, or leave it
out, and then it fails with whatever its `?` and `fail` let through:

```tondo
import std.console
import std.fs

fn main(): !fs.FsError {
    let text = fs.readText("notes.txt")?
    console.println("{text.lines().length()} lines")
}
```

```tondo
import std.console
import std.fs
import std.json

type Settings = { name: String }

fn main() {
    let text = fs.readText("settings.json")?
    let settings = json.decode[Settings](text)?
    console.println(settings.name)
}
```

The second `main` propagates two kinds of error and declares neither. Only
`main` and a script may do that, because nothing imports their signature;
every other function says what it fails with ([Errors](07-errors.md)).

There is one `main` in a program, in its root file. It is not `pub`, not
generic and not a method, and nothing calls it.

## How a program ends

| The program | Exits with | And writes to standard error |
| --- | --- | --- |
| reaches the end of `main` | 0 | nothing |
| fails: `main` lets an error out | 1 | `error: ` and the error |
| panics: a bug | 101 | the panic's report, with its stack |
| calls `process.exit(code)` | that code | nothing |
| is interrupted with Ctrl-C or `SIGTERM` | 130 or 143 | nothing |

An error is shown by its `Display` when it has one, and by its variant's name
when it is an enum without it. This program ends with `error: Stop.Missing`
and status 1:

```tondo
enum Stop {
    Missing(String)
}

fn main() {
    fail Stop.Missing("config")
}
```

A signal does not kill a Tondo program: it cancels it. Every wait ends, each
task runs its `defer`s, and then the program exits. A second signal ends it
at once. [Tasks](13-tasks.md) explains cancellation.

## A script

A file needs no `main`. When its top level has statements, they are the
program, in order:

```tondo
#!/usr/bin/env tondo
import std.console
import std.env

fn shout(text: String): String {
    "{text.toUpper()}!"
}

let words = env.args()
if words.isEmpty() {
    console.println("nothing to shout")
}
for word in words {
    console.println(shout(word))
}
```

`tondo shout.to hello world` runs it, and with the first line and
`chmod +x shout.to`, so does `./shout.to hello world`.

- Declarations may go anywhere in the file: types, functions, constants,
  imports.
- `let`, `var`, loops, calls and `defer` at the top level are the statements
  of an implicit `main`, and `?`, `fail` and `return` act on it.
- A function does not see the script's bindings. It takes what it needs as
  parameters, and the compiler says so when one tries:

```text
error[E1001]: unknown name `limit`; `limit` is a binding of the script, which its functions do not see: pass it to the function as a parameter
```

- A file with statements cannot be imported, and a file cannot have both
  statements and a `main`.
- A project's `src/main.to` may be a script too.

Only the root file runs anything. Importing a module never executes code:
there are no initializers that run before `main`, and so the order in which
a program starts is the order written in it.

## Lines and statements

A new line ends a statement. There are no semicolons, and writing one is an
error that says so:

```tondo error E0002
fn main() {
    let x = 1;
}
```

```text
error[E0002]: `;` is not a Tondo token: a new line ends a statement
```

A statement goes on to the next line when it is plainly unfinished:

- inside `(...)` and `[...]`;
- after `,`, `.`, `:`, `=`, `=>`, or a binary operator such as `+`, `and`,
  `==`, `..`;
- when the next line starts with `.`, or with a binary operator other than
  `-`.

```tondo
import std.console

fn main() {
    let total =
        1 +
        2
    let text = "  one line, several calls  "
        .trim()
        .toUpper()
        .replace(",", ";")
    let labels = [
        "first",
        "second",
    ]
    console.println("{total} {text} {labels}")
}
```

```text output
3 ONE LINE; SEVERAL CALLS [first, second]
```

A trailing comma is allowed in every list. `else` goes on the line of the
`}` before it, since a new line there would end the `if`:

```text
error[E0004]: `else` goes on the line of the `}` before it: `} else {`
```

None of this is a matter of taste for long: `tondo fmt` rewrites a file in
the one canonical layout, with four spaces of indentation, and has no
options. Write compactly and let it format.

## The value of a block

A block's last expression is its value. That is how a function gives its
result, how `if` and `match` are expressions, and why `return` is only for
leaving early:

```tondo
import std.console

fn describe(n: Int): String {
    let size = if n > 100 { "large" } else { "small" }
    let sign = match n {
        0 => "zero"
        _ => if n < 0 { "negative" } else { "positive" }
    }
    "{size} and {sign}"
}

fn main() {
    console.println(describe(250))
    console.println(describe(-3))
}
```

```text output
large and positive
small and negative
```

A statement whose value nobody uses must do something. A bare value such as
`3 + 4` on a line of its own is an error (`E1306`), and so is a result or an
option that is dropped, since it may hold a failure (`E1303`). To evaluate
something and discard it on purpose, assign it to `_`:

```tondo
import std.console

fn log(message: String): Bool {
    console.println(message)
    true
}

fn main() {
    _ = log("started")
}
```

## Comments and documentation

```tondo
import std.console

// A comment to the end of the line.

/*
    A block comment, which may hold another:
    /* as this one */
*/

/// The area of a rectangle.
///
/// Documentation is Markdown, and belongs to the declaration that follows.
pub fn area(width: Float, height: Float): Float {
    width * height
}

fn main() {
    console.println("{area(2.0, 3.5)}")
}
```

```text output
7.0
```

A `///` comment is the documentation of what follows it: a function, a
type, a field, a variant, a constant. The tools read it. `tondo api` shows
it with the declaration, an editor shows it on hover, `std.cli` makes a
command's help of it, and `std.http` writes it into an OpenAPI document.
`tondo check --docs` lists what is public and has none.

## Names

A name is letters, digits and `_`, in any script Unicode has, and capitals
matter: `user`, `User` and `USER` are three names. Capitals never change
what a name means or who sees it. Only `pub` exports.

The conventions are checked, and a name that breaks one is a warning with
the name it should have:

| What | Is written | As |
| --- | --- | --- |
| Types, traits, enum variants, constants | `PascalCase` | `HttpClient`, `Shape.Circle`, `MaxRetries` |
| Functions, methods, fields, parameters, bindings | `camelCase` | `findUser`, `userId` |
| Modules | lower case | `std.http`, `app.models` |
| Acronyms | as words | `JsonValue`, `userId`, never `JSONValue` or `userID` |

```text
warning[W1004]: `user_name` should be `userName`: functions, fields, bindings and modules are `camelCase`, with acronyms written as words
```

`_` alone is not a name: it discards. It stands for a parameter that is not
used, a part of a pattern that does not matter, or a value thrown away with
`_ = call()`. A binding named `_name` is one the code keeps but does not
read, which the unused-binding warning then leaves alone.

A keyword cannot be a name, with one exception: it can be a field, because a
field's place is never ambiguous. Data from outside often has a `type`:

```tondo
import std.console

type Event = {
    type: String
    target: String
}

fn main() {
    let event = Event { type: "click", target: "save" }
    console.println("{event.type} on {event.target}")
}
```

```text output
click on save
```

## Keywords

```text
alias      and        as         break      const
continue   defer      derive     else       enum
extern     fail       false      fn         for
if         impl       import     in         let
match      mut        none       not        or
priv       pub        ref        return     select
self       spawn      suite      test       trait
true       type       unsafe     var        with
```

A few more words mean something only in one place, and are ordinary names
everywhere else:

| Word | Where it is special |
| --- | --- |
| `ok`, `err`, `some` | followed by `(`: they build and match a `Result` or an `Option` |
| `recover` | before `{`: it stops a panic |
| `within` | before a duration and a block: it sets a deadline |
| `context`, `annotation` | at the start of a declaration, before its name |
| `try` | before `{`: `?` inside ends the block, not the function |

`ok`, `err` and `some` can still not name a function, since `ok(...)` must
always mean the same thing. `Some`, `None`, `Ok` and `Err` are not Tondo:
writing one gives an error with the lower-case form.

These are not keywords, and may be used as names: `catch`, `class`,
`dynamic`, `extends`, `finally`, `macro`, `new`, `null`, `override`,
`protected`, `static`, `throw`, `while`, `yield`. Tondo has none of what
they stand for elsewhere; [What is not there](19-design.md) says what it has
instead.

## The order things are evaluated in

Always left to right, and the same in every build:

- a receiver before its arguments, and the arguments in the order written;
- the elements of an array, a map or a record in the order written, and in a
  map's entry the key before the value;
- the left operand before the right one;
- the expressions of an interpolated string in the order they appear.

```tondo
import std.console

fn say(text: String): Int {
    console.println(text)
    text.length()
}

fn main() {
    let total = say("first") + say("second") * say("third")
    console.println("{total}")
}
```

```text output
first
second
third
35
```

The multiplication is done before the addition, as arithmetic says, but its
operands were evaluated in the order they are written. An optimized build
may skip or merge work only when nothing the program does can tell.

Next: [Values and bindings](02-values.md).
