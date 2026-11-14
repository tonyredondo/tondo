# std.cli

A command line read into the program's types, and what a program writes to
a terminal. A record is a command: its fields are the options and
arguments, their types say what is accepted, their documentation is the
help. An enum is subcommands. There is no builder to call and no strings to
keep in step with the code.

- [A command from a type](#a-command-from-a-type)
- [Subcommands](#subcommands)
- [Writing to a terminal](#writing-to-a-terminal)
- [What to reach for](#what-to-reach-for)

## A command from a type

```tondo
import std.cli
import std.console
import std.time

/// Copies files to a server.
type Upload = {
    /// The files to copy.
    @cli.Argument
    files: Array[String]
    /// Where to copy them.
    @cli.Option(short: "t", env: ["UPLOAD_TARGET"])
    target: String = "staging"
    /// How long to wait for the server.
    timeout: time.Duration = time.seconds(30)
    /// Says what it would do, and does nothing.
    dryRun: Bool
}

fn main() {
    for line in [["a.txt", "b.txt", "-t", "production", "--timeout", "2m"], ["--dry-run", "a.txt"], ["--port", "80"]] {
        match cli.parseArgs[Upload](line) {
            ok(upload) => console.println("{upload.files} to {upload.target}, {upload.timeout}, dry run: {upload.dryRun}")
            err(error) => console.println("{error}".lines().first() ?? "")
        }
    }
}
```

```text output
[a.txt, b.txt] to production, 2m0s, dry run: false
[a.txt] to staging, 30s, dry run: true
error: unknown option `--port`; did you mean `--target`?
```

- A field is an option, `--kebab-case` from its name, or with
  `@cli.Argument` a positional argument. `Bool` is a flag, `T?` is optional,
  `Array[T]` repeats, an enum without data is a choice, and a
  `time.Duration` is written `30s` or `1m30s`.
- A field's default is the option's, and the help shows it. `env` names
  variables of the environment to read before the default.
- [`parse`](#parse) reads the program's own arguments, prints the help for
  `-h`, and on a wrong line prints what is wrong and ends with status 2.
  [`parseArgs`](#parseargs) gives the error instead, as above.

## Subcommands

An enum is a choice of commands, each variant with its own options, and a
record can hold options that all of them share:

```tondo
import std.cli
import std.console

/// Keeps notes.
type Notes = {
    /// The directory of the notes.
    @cli.Option(short: "d")
    directory: String = "notes"
    @cli.Argument
    command: Command
}

enum Command {
    /// Adds a note.
    Add {
        @cli.Argument
        text: String
    }
    /// Lists the notes, the newest first.
    List {
        @cli.Option(short: "n", default: "10")
        limit: Int
    }
}

fn main() {
    for line in [["add", "buy tea"], ["list", "-n", "3", "-d", "work"]] {
        match cli.parseArgs[Notes](line) {
            ok(notes) => match notes.command {
                Add { text } => console.println("add `{text}` to {notes.directory}")
                List { limit } => console.println("list {limit} of {notes.directory}")
            }
            err(error) => console.println("{error}")
        }
    }
}
```

```text output
add `buy tea` to notes
list 3 of work
```

## Writing to a terminal

```tondo
import std.cli
import std.console

type Row = { service: String, requests: Int, errors: Int }

fn main() {
    let rows = [
        Row { service: "api", requests: 18204, errors: 12 },
        Row { service: "worker", requests: 932, errors: 0 },
    ]
    console.println(cli.table(rows))
    console.println(cli.table(rows, columns: ["service", "errors"]))
}
```

```text output
┌─────────┬──────────┬────────┐
│ service │ requests │ errors │
├─────────┼──────────┼────────┤
│ api     │    18204 │     12 │
│ worker  │      932 │      0 │
└─────────┴──────────┴────────┘

┌─────────┬────────┐
│ service │ errors │
├─────────┼────────┤
│ api     │     12 │
│ worker  │      0 │
└─────────┴────────┘
```

A table's columns are a record's fields, with numbers to the right.
[`markup`](#markup) styles text, `cli.markup("[bold red]failed[/]")`, and
writes plain text when the output is not a terminal.
[`progress`](#progress) shows work as it advances, and [`ask`](#ask),
[`confirm`](#confirm), [`choose`](#choose) and [`secret`](#secret) put a
question to whoever is at the keyboard.

## What to reach for

| To | Use |
| --- | --- |
| Read the command line into a type | [`parse`](#parse), [`parseArgs`](#parseargs) |
| Say how a field is written | [`Argument`](#argument), [`Option`](#option), [`Rest`](#rest) |
| Print the help, or a shell's completions | [`help`](#help), [`completions`](#completions) |
| Show rows | [`table`](#table) |
| Style text | [`markup`](#markup) |
| Show progress | [`progress`](#progress) |
| Ask | [`ask`](#ask), [`confirm`](#confirm), [`choose`](#choose), [`secret`](#secret) |
| Know the terminal | [`isTerminal`](#isterminal), [`width`](#width) |

The [`logstat`](../../examples/logstat) and
[`warehouse`](../../examples/warehouse) examples are command lines with
subcommands.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.cli` shows the same in a terminal.

- [Functions](#functions): [`parse`](#parse), [`parseArgs`](#parseargs), [`help`](#help), [`completions`](#completions), [`isTerminal`](#isterminal), [`width`](#width), [`markup`](#markup), [`table`](#table), [`progress`](#progress), [`ask`](#ask), [`confirm`](#confirm), [`choose`](#choose), [`secret`](#secret)
- [Argument](#argument)
- [Option](#option)
- [Rest](#rest)
- [CliError](#clierror)
- [Shell](#shell)
- [Progress](#progress-1): [`Progress.advance`](#progressadvance), [`Progress.finish`](#progressfinish)

### Functions

#### parse

```tondo
pub fn parse[T: reflect.Reflect](): T
```

The command line of the program read as a `T`: writes the help asked
for and ends with 0, or writes the error and the usage to standard error
and ends with 2.

```tondo
import std.cli
import std.console

/// Manages the notes.
enum Notes {
    /// Adds a note.
    Add(Note)
    /// Lists the notes.
    List
}

type Note = {
    /// What it says.
    text: String
}

match cli.parse[Notes]() {
    Notes.Add(note) => console.println("added {note.text}")
    Notes.List => console.println("no notes yet")
}
```

#### parseArgs

```tondo
pub fn parseArgs[T: reflect.Reflect](args: Array[String]): T ! CliError
```

The arguments, without the program's name, read as a `T`: what `parse`
does, with the failure to handle, as a test or a program that goes on
wants it. `--help` is a failure too, whose `help` is `true`.

```tondo
import std.cli
import std.console

type Options = {
    /// Who to greet.
    name: String = "world"
}

match cli.parseArgs[Options](["--name", "Ada"]) {
    ok(options) => console.println("hello {options.name}")
    err(error) => console.eprintln(error.message)
}
```

#### help

```tondo
pub fn help[T: reflect.Reflect](): String
```

The help of the command `T`.

```tondo
import std.cli
import std.console

/// Greets someone.
type Options = {
    /// Who to greet.
    name: String = "world"
}

console.print(cli.help[Options]())
```

#### completions

```tondo
pub fn completions[T: reflect.Reflect](shell: Shell): String
```

The script that completes the program's command line in `shell`, to
print for the shell to load.

```tondo
import std.cli
import std.console

type Options = {
    /// Who to greet.
    name: String = "world"
}

console.print(cli.completions[Options](cli.Shell.Bash))
```

#### isTerminal

```tondo
pub fn isTerminal(): Bool
```

Whether the console's output is a terminal: `false` when it goes to a
pipe or a file, where a question or a bar has nobody to see it.

```tondo
import std.cli
import std.console

if cli.isTerminal() {
    console.println(cli.markup("[green]ready[/]"))
} else {
    console.println("ready")
}
```

#### width

```tondo
pub fn width(): Int
```

The columns of the terminal of the console's output, or 80 without one.

```tondo
import std.cli
import std.console

console.println("─".repeat(cli.width()))
```

#### markup

```tondo
pub fn markup(text: String): String
```

A text with styles, to print: `[bold red]error[/]` gives `error` in bold
red, and what follows as it was; blocks nest, and `[[` and `]]` give
brackets. With an interpolated string, its values stay as they are. No
codes off a terminal or with `NO_COLOR`.

It writes nothing itself: print what it gives.

```tondo
import std.cli
import std.console

let name = "Ada [admin]"
console.println(cli.markup("[bold red]error:[/] no user [yellow]{name}[/]"))
console.println(cli.markup("[black on #ffcc00] note [/] use [[brackets]] so"))
```

#### table

```tondo
pub fn table[T: reflect.Reflect](rows: Array[T], columns: Array[String] = []): String
```

A table with a border, to print, with the numbers to the right. Its rows
are arrays, written as they are, with `columns` over them as headers and
no headers without it; a short row leaves the rest of its cells empty:

```tondo
import std.cli
import std.console

let rows = [["Ada", "36"], ["Grace", "120"]]
console.print(cli.table(rows, columns: ["Name", "Age"]))
// ┌───────┬─────┐
// │ Name  │ Age │
// ├───────┼─────┤
// │ Ada   │  36 │
// │ Grace │ 120 │
// └───────┴─────┘
```

Or records, a column a field, or those `columns` names in their order,
with the name of the field over it:

```tondo
import std.cli
import std.console

type User = { name: String, age: Int, email: String? }

let users = [User { name: "Ada", age: 36, email: none }]
console.print(cli.table(users))
console.print(cli.table(users, columns: ["age", "name"]))
```

A cell can have styles, from `markup`:

```tondo
import std.cli
import std.console

let up = cli.markup("[green]up[/]")
let down = cli.markup("[red]down[/]")
console.print(cli.table([["api", up], ["db", down]], columns: ["Service", "State"]))
```

#### progress

```tondo
pub fn progress(label: String, total: Int? = none): Progress
```

A progress of the work `label` of `total` steps, drawn now.

```tondo
import std.cli

let bar = cli.progress("Building", total: 3)
bar.advance()
bar.advance(by: 2)
bar.finish()

let spinner = cli.progress("Waiting")
spinner.advance()
spinner.finish()
```

#### ask

```tondo
pub fn ask(question: String, default: String = ""): String
```

The answer to a question, or `default` when it is left empty or the input
has ended.

```tondo
import std.cli
import std.console

let name = cli.ask("Name")
let city = cli.ask("City", default: "Lima")
console.println("{name} from {city}")
```

#### confirm

```tondo
pub fn confirm(question: String, default: Bool = false): Bool
```

`true` for `y` or `yes`, `false` for `n` or `no`, `default` when it is
left empty or the input has ended; anything else asks again.

```tondo
import std.cli
import std.console

if not cli.confirm("Start the server?") {
    console.println(cli.markup("[red]Server not started.[/]"))
    return
}
```

#### choose

```tondo
pub fn choose(question: String, options: Array[String]): Int
```

The index in `options` of the one chosen by its number, from 1 as they
are shown; anything else asks again, and the first is taken when the
input has ended.

```tondo
import std.cli
import std.console

let drinks = ["tea", "coffee"]
let chosen = cli.choose("Pick one", drinks)
console.println("a {drinks[chosen]}")
```

#### secret

```tondo
pub fn secret(question: String): String
```

An answer read without showing it, when the input is a terminal.

```tondo
import std.cli

let password = cli.secret("Password")
_ = password
```

### Argument

```tondo
pub annotation Argument = {
    /// The value when it is missing, written as on the command line.
    default: String? = none
}
```

Makes a field a positional argument, in the order of the fields.

Implements `reflect.Reflect`.

### Option

```tondo
pub annotation Option = {
    short: String? = none
    default: String? = none
    env: Array[String] = []
    hidden: Bool = false
}
```

How a field is written as an option: its letter, its value when it is
missing, written as on the command line, the variables of the
environment read before it, the first that is set, and whether the help
leaves it out.

Implements `reflect.Reflect`.

### Rest

```tondo
pub annotation Rest = {
    /// What the help calls them.
    name: String = "args"
}
```

Makes a field, an `Array[String]`, take what the command does not read:
the arguments after `--`, the options it does not declare and the
arguments past its own, as written, to pass them on to another program.

Implements `reflect.Reflect`.

### CliError

```tondo
pub type CliError = {
    message: String
    help: Bool
}
```

Why a command line was not read, with the usage of its command, or the
help that it asked for.

```tondo
import std.cli
import std.console

type Options = { count: Int = 1 }

match cli.parseArgs[Options](["--count", "many"]) {
    ok(options) => console.println("{options.count}")
    err(error) => console.println("{error.message}")
}
```

Implements `Display`, `reflect.Reflect`.

### Shell

```tondo
pub enum Shell {
    Bash
    Zsh
    Fish
}
```

A shell whose completion `completions` writes.

```tondo
import std.cli
import std.console

type Options = { verbose: Bool = false }

// What `myapp completions zsh` prints, for the shell to load.
console.print(cli.completions[Options](cli.Shell.Zsh))
```

### Progress

```tondo
pub type Progress = {
    // private fields
}
```

A bar of the progress of some work, or a spinner when it has no total,
that the console's output shows on a terminal.

```tondo
import std.cli

let files = ["a.txt", "b.txt", "c.txt"]
let bar = cli.progress("copying", total: some(files.length()))
for _ in files {
    bar.advance()
}
bar.finish()
```

#### Progress.advance

```tondo
pub fn Progress.advance(self, by: Int = 1)
```

Counts `by` more steps done.

#### Progress.finish

```tondo
pub fn Progress.finish(self)
```

Ends the progress: a full bar on a terminal, or one line without one.
