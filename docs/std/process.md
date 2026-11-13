# std.process

Running other programs, and what this one is to the system. A command is
written as in a shell, `$(git log -n {count} | head -5)`, and no shell runs
it: the compiler splits the line into a program and its arguments when it
compiles, so a value put in it is always one argument, whatever it holds, and
a file name with a space or a `$` in it is only a file name. Running one
waits without holding a thread.

- [Writing a command](#writing-a-command)
- [How it ended, input and output](#how-it-ended-input-and-output)
- [What to reach for](#what-to-reach-for)

## Writing a command

```tondo
import std.console
import std.process

fn main(): !process.ProcessError {
    // The words of a command, written as in a shell. A value is always one argument.
    let name = "a file with spaces"
    let listing = $(printf '%s\n' tea {name} '$HOME').lines()?
    console.println("{listing}")

    // A pipeline, with no shell in between.
    let words = ["tea", "cup", "tea", "pot", "tea"]
    let counted = $(printf '%s\n' {...words} | sort | uniq -c | sort -rn).lines()?
    console.println("{counted.map((line) { line.trim() })}")

    // `text` gives what it printed, without the line's end; `run` shows it and fails when the command fails.
    console.println($(LANG=C date -u -d @0 +%Y).text()?)
    match $(false).run() {
        ok(_) => console.println("it worked")
        err(error) => console.println("{error}")
    }

    // A command that arrives as text while the program runs.
    let line = "printf 'from a %s\n' string"
    console.println(process.parse(line)?.text()?)
}
```

```text output
[tea, a file with spaces, $HOME]
[3 tea, 1 pot, 1 cup]
1970
exited with code 1
from a string
```

What the text between `$(` and `)` holds:

| | Example | Meaning |
| --- | --- | --- |
| A word | `-la`, `--date=short` | one argument |
| `'...'` | `'*.to'` | text as it is, with nothing looked at inside |
| `"..."` | `"fix: {title}"` | one argument, with interpolation |
| `{value}` | `{dir}`, `--out={path}` | one argument or a part of one, never split again |
| `{...values}` | `{...files}` | an argument for each element |
| `\|` | `sort \| uniq -c` | what a stage writes is what the next reads |
| `<`, `>`, `>>` | `< {input}`, `>> out.txt` | the first stage reads a file; the last writes one, or adds to it |
| `2>`, `2>>`, `2>&1` | `2> errors.log` | a stage's errors, to a file or to where its output goes |
| `NAME=value` first | `LANG=C sort` | a variable of that stage's environment |

It is not a shell, and says so where a shell's habits are written, with what
to write instead:

```text
error[E0008]: `$HOME` reads a variable of the shell, which runs none: write `{env.get("HOME") ?? ""}`, or quote the `$`
error[E0008]: `*` matches files in a shell, which runs none: write `{...fs.glob("*.md")?}`, or quote it when the character is meant: `'*'`
error[E0008]: `&&` runs a command when another succeeds, which a shell does: write `.run()?` for each
```

A command comes from one of three places, and all three give the same
value, which joins others with `|`:

| The command | Written |
| --- | --- |
| is known when the program is written | `$(git log -n {count} \| head -5)` |
| arrives as text while the program runs | [`process.parse(line)?`](#parse) |
| is a program and its arguments, as values | [`process.command(program, ...arguments)`](#command) |

## How it ended, input and output

```tondo
import std.console
import std.process

fn main(): !process.ProcessError {
    // `status` gives how it ended and never fails for that; `run`, `text`, `lines` and `check` fail.
    let found = $(test -d /).status()?
    let missing = $(test -d /no/such/place).status()?
    console.println("{found.success} {missing.success} {missing.code}")
    match $(no-such-program).status() {
        ok(_) => console.println("it ran")
        err(_) => console.println("there is no such program")
    }

    // Where it reads from and writes to, as a shell writes it.
    let folder = "/tmp"
    $(printf 'b\na\n' > {folder}/letters.txt).run()?
    console.println("{$(sort < {folder}/letters.txt).lines()?}")

    // A program and its arguments as values, for when they are not known while writing.
    let program = "printf"
    let arguments = ["%s-%s\n", "left", "right"]
    console.println(process.command(program, ...arguments).text()?)
    let here = $(pwd).directory("/").env("LANG", "C").text()?
    console.println(here)
}
```

```text output
true false 1
there is no such program
[a, b]
left-right
/
```

| | |
| --- | --- |
| [`run`](#run) | with this program's input and output: what the user sees. Fails when the command fails |
| [`text`](#text), [`lines`](#lines) | what it printed, as text without the line's end, or by lines. Fail when the command fails |
| [`status`](#status) | without input, discarding output: only how it ended, which is never a failure |
| [`output`](#output) | captures what it printed, and how each stage ended |
| [`check`](#check) | as `output`, failing when the command failed |
| [`start`](#start) | starts it and gives a [`Process`](#process) to write to, read from, wait for or stop |

- A command that fails is `ProcessError.Failed`, with how each stage ended,
  so a script stops at the first command that goes wrong and a program that
  wants the exit code matches that error.
- [`Command.directory`](#commanddirectory), [`Command.env`](#commandenv)
  and [`Command.stdin`](#commandstdin) say where it runs, with what
  environment and what input, for what a line cannot say.
- Cancelling the task that waits for a command, as a `within` does, ends
  the command. [`Command.grace`](#commandgrace) asks it to end first and
  stops it only after a while, as `docker stop` does, and
  [`Command.group`](#commandgroup) makes that reach everything it started.
- [`shell`](#shell) runs a line through the system's shell, for when a real
  shell is what is wanted.

## What to reach for

| To | Use |
| --- | --- |
| Run a program | `$(...)`, then [`run`](#run), [`text`](#text), [`lines`](#lines), [`status`](#status), [`output`](#output) or [`check`](#check) |
| Join programs | `\|` inside `$(...)`, or `first \| second` between two commands |
| Run a line that came from a file or from the user | [`parse`](#parse) |
| Talk to a program as it runs | [`start`](#start), [`Process.stdin`](#processstdin), [`Process.stdout`](#processstdout), [`Process.wait`](#processwait) |
| Find a program | [`which`](#which) |
| End this program | [`exit`](#exit) |
| Be told of Ctrl-C or a request to stop | [`signals`](#signals) |
| Become another program | [`Command.exec`](#commandexec) |
| Split a line as a shell would, or quote for one | [`splitWords`](#splitwords), [`quoted`](#quoted) |

The program's own arguments and environment are in [`std.env`](env.md).
The scripts [`authors.to`](../../examples/scripts/authors.to) and
[`rerun.to`](../../examples/scripts/rerun.to) run other programs.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.process` shows the same in a terminal.

- [Functions](#functions): [`command`](#command), [`shell`](#shell), [`exit`](#exit), [`signals`](#signals), [`run`](#run), [`status`](#status), [`output`](#output), [`check`](#check), [`start`](#start), [`text`](#text), [`lines`](#lines), [`which`](#which), [`signal`](#signal), [`splitWords`](#splitwords), [`parse`](#parse), [`quoted`](#quoted)
- [Signal](#signal-1)
- [Input](#input)
- [Output](#output-1)
- [ExitStatus](#exitstatus)
- [ProcessOutput](#processoutput): [`ProcessOutput.text`](#processoutputtext)
- [ProcessError](#processerror)
- [Process](#process): [`Process.stdin`](#processstdin), [`Process.stdout`](#processstdout), [`Process.stderr`](#processstderr), [`Process.id`](#processid), [`Process.wait`](#processwait), [`Process.output`](#processoutput-1), [`Process.check`](#processcheck), [`Process.cancel`](#processcancel), [`Process.detach`](#processdetach), [`Process.signal`](#processsignal)
- [Command](#command-1): [`Command.directory`](#commanddirectory), [`Command.env`](#commandenv), [`Command.removeEnv`](#commandremoveenv), [`Command.clearEnv`](#commandclearenv), [`Command.stdin`](#commandstdin), [`Command.stdout`](#commandstdout), [`Command.stderr`](#commandstderr), [`Command.mergeStderr`](#commandmergestderr), [`Command.group`](#commandgroup), [`Command.grace`](#commandgrace), [`Command.invokedAs`](#commandinvokedas), [`Command.exec`](#commandexec)
- [Pipeline](#pipeline): [`Pipeline.stdin`](#pipelinestdin), [`Pipeline.stdout`](#pipelinestdout), [`Pipeline.mergeStderr`](#pipelinemergestderr), [`Pipeline.group`](#pipelinegroup), [`Pipeline.grace`](#pipelinegrace)

### Functions

#### command

```tondo
pub fn command(program: String, arguments: ...String): Command
```

A program and its arguments, each on its own: no shell, no globbing.

```tondo
import std.console
import std.process

// `|` joins commands into a pipeline, without a shell.
let counted = (process.command("ls", "-l") | process.command("wc", "-l")).output()?
console.println("{counted.text()?.trim()} lines")
```

#### shell

```tondo
pub fn shell(text: String): Command
```

A command that the system's shell runs: `/bin/sh -c text`.

#### exit

```tondo
pub fn exit(code: Int): Never
```

Delivers pending console output and ends the program, without `defer`s.

#### signals

```tondo
pub fn signals(wanted: ...Signal): Receiver[Signal]
```

A channel of the signals named, which no longer cancel or end the
program: it decides, as reloading its settings on `Hangup`. Two equal
ones the channel has not given yet are one.

#### run

```tondo
pub fn run(self): !ProcessError
```

On a Command or a Pipeline: with the program's streams, waits, and
fails with `ProcessError.Failed`, the status of each stage inside and
nothing captured, when a stage does not end well.

#### status

```tondo
pub fn status(self): ExitStatus ! ProcessError
```

On a Command or a Pipeline: without input, discarding output.

#### output

```tondo
pub fn output(self): ProcessOutput ! ProcessError
```

On a Command or a Pipeline: captures standard output and error.

#### check

```tondo
pub fn check(self): ProcessOutput ! ProcessError
```

On a Command or a Pipeline: as `output`, failing when a stage fails.

#### start

```tondo
pub fn start(self): Process ! ProcessError
```

On a Command or a Pipeline: starts without waiting.

#### text

```tondo
pub fn text(self): String ! ProcessError
```

On a Command or a Pipeline: as `check`, its standard output as UTF-8,
without the new lines at its end.

#### lines

```tondo
pub fn lines(self): Array[String] ! ProcessError
```

On a Command or a Pipeline: as `text`, by lines; none for no output.

#### which

```tondo
pub fn which(name: String): String?
```

The file a shell runs for `name`: `name` itself when it holds a `/`,
or else the first file of that name in the directories of `PATH` that
may be run; `none` without one.

#### signal

```tondo
pub fn signal(pid: Int, sent: Signal): !ProcessError
```

Sends a signal to the process `pid`, one the program did not start:
`ProcessError.NotFound` when there is none, `PermissionDenied` when
the program may not signal it.

#### splitWords

```tondo
pub fn splitWords(text: String): Array[String]?
```

The words of a command line as a POSIX shell splits them, with its
quotes and backslashes, but expanding nothing: `go test -run 'A B'` is
`["go", "test", "-run", "A B"]`. `none` when a quote is not closed or
the text ends in a lone backslash.

#### parse

```tondo
pub fn parse(text: String): Pipeline ! ProcessError
```

The command of a line that arrives while the program runs, read as
`$( )` reads its text (language 20.5): words, `'...'` and `"..."`, `|`,
`<`, `>`, `>>`, `2>`, `2>>`, `2>&1` and `NAME=value` first, but `{` and
`}` are plain characters. A pipeline, of one stage when there is one.
What a shell does and no shell runs here, as `$HOME`, `*` or `&&`, is
`ProcessError.InvalidCommand`, which says what.

```tondo
import std.console
import std.process

let counted = process.parse("printf 'b\\na\\nb\\n' | sort | uniq -c")?.lines()?
console.println("{counted.length()} lines")
```

#### quoted

```tondo
pub fn quoted(words: Array[String]): String
```

The words as one command line a POSIX shell splits back into them: a
word with any character but letters, digits and `_@%+=:,./-` goes in
single quotes.

### Signal

```tondo
pub enum Signal {
    Interrupt
    Terminate
    Hangup
    User1
    User2
    /// Ctrl-\: as `Interrupt`, but ending a program that does not listen
    /// with a dump of its memory, by the system's default.
    Quit
    /// Ends a process at once, without letting it answer: it is sent and
    /// never received, so `signals` leaves it out.
    Kill
}
```

The signals a program receives. Without `signals`, `Interrupt` (Ctrl-C)
and `Terminate` cancel the program: its tasks run their `defer`s and it
ends with 130 or 143; a second one ends it at once.

```tondo
import std.console
import std.process

// Hangup reloads the settings; Interrupt and Terminate still end the program.
for signal in process.signals(process.Signal.Hangup) {
    if signal == process.Signal.Hangup {
        console.println("reloading")
    }
}
```

### Input

```tondo
pub enum Input {
    Inherit
    Null
    Text(String)
    Bytes(Array[Byte])
    File(String)
    Reader(io.Reader)
    Pipe
}
```

Where a process reads its standard input from: the program's (`Inherit`),
nothing (`Null`), a fixed text or bytes, a file (`<`), a stream of
`std.io`, or the program through the `Process` (`Pipe`).

```tondo
import std.console
import std.process

let sorted = process.command("sort").stdin(process.Input.Text("b\na\n")).output()?
console.print(sorted.text()?)
```

### Output

```tondo
pub enum Output {
    Inherit
    Null
    Capture
    File(String)
    Append(String)
    Writer(io.Writer)
    Stdout
    Pipe
}
```

Where a process writes its output: the program's (`Inherit`), nowhere
(`Null`), into the `ProcessOutput` (`Capture`), a file (`>`), the end of
one (`>>`), a stream of `std.io`, standard output (`Stdout`, for standard
error: `2>&1`), or the program through the `Process` (`Pipe`).

```tondo
import std.process

// Standard error into a file, and standard output as it is.
process.command("make").stderr(process.Output.File("build.log")).run()?
```

### ExitStatus

```tondo
pub type ExitStatus = {
    code: Int
    success: Bool
}
```

How a process ended: its exit code, `128 + signal` when a signal ended
it, and whether it succeeded; a pipeline succeeds when every stage does.

```tondo
import std.console
import std.process

let status = process.command("test", "-d", "/tmp").status()?
console.println("{status.success} {status.code}")
```

### ProcessOutput

```tondo
pub type ProcessOutput = {
    stdout: Array[Byte]
    stderr: Array[Byte]
    combined: Array[Byte]
    statuses: Array[ExitStatus]
}
```

What a process wrote: its standard output, its standard error, both in
the order they came, and the status of each stage.

```tondo
import std.console
import std.process

let listed = process.command("ls", "-la").output()?
console.println("{listed.statuses[0].code}: {String.fromBytes(listed.stdout)?.lines().length()} lines")
```

#### ProcessOutput.text

```tondo
pub fn ProcessOutput.text(self): String ! ProcessError
```

Standard output as UTF-8.

### ProcessError

```tondo
pub enum ProcessError {
    NotFound
    PermissionDenied
    InvalidData
    /// From `check`: a stage did not succeed.
    Failed(ExitStatus, ProcessOutput)
    Host
    /// From `parse`: the text is no command, and why, as a `$HOME` that
    /// no shell expands.
    InvalidCommand(String)
}
```

Why a process could not run: its program was not found or not allowed,
its output was not UTF-8, a stage failed under `check`, or the host
failed otherwise.

```tondo
import std.console
import std.process

match process.command("false").check() {
    ok(_) => console.println("it worked")
    err(process.ProcessError.Failed(status, _)) => console.println("it ended with {status.code}")
    err(error) => console.println("{error}")
}
```

Implements `reflect.Reflect`.

### Process

```tondo
pub type Process
```

A process started and not yet waited for.

```tondo
import std.console
import std.process

let child = process.command("cat").stdin(process.Input.Pipe).stdout(process.Output.Capture).start()?
child.stdin().writeLine("through the pipe")?
child.stdin().close()?
console.print(child.output()?.text()?)
```

#### Process.stdin

```tondo
pub fn Process.stdin(self): io.Writer
```

The process's standard input, when it was started with `Input.Pipe`.

#### Process.stdout

```tondo
pub fn Process.stdout(self): io.Reader
```

The process's standard output, when it was started with `Output.Pipe`.

#### Process.stderr

```tondo
pub fn Process.stderr(self): io.Reader
```

The process's standard error, when it was started with `Output.Pipe`.

#### Process.id

```tondo
pub fn Process.id(self): Int
```

The system's identifier of the process, of the last stage for a
pipeline.

#### Process.wait

```tondo
pub fn Process.wait(self): ExitStatus ! ProcessError
```

Waits until every stage ends and gives the status.

#### Process.output

```tondo
pub fn Process.output(self): ProcessOutput ! ProcessError
```

Waits until every stage ends and gives what it captured.

#### Process.check

```tondo
pub fn Process.check(self): ProcessOutput ! ProcessError
```

As `output`, failing with `ProcessError.Failed` when a stage did not
succeed.

#### Process.cancel

```tondo
pub fn Process.cancel(self): ExitStatus ! ProcessError
```

Ends every stage, and with `group()` every process they started, and
waits for them.

#### Process.detach

```tondo
pub fn Process.detach(self)
```

Lets it go on without waiting, also after the program ends.

#### Process.signal

```tondo
pub fn Process.signal(self, signal: Signal)
```

Sends the signal to every stage, and with `group()` to every process
they started; `cancel` ends them and waits.

### Command

#### Command.directory

```tondo
pub fn Command.directory(self, path: String): Command
```

The command run in the directory `path`.

#### Command.env

```tondo
pub fn Command.env(self, name: String, value: String): Command
```

The command with the environment variable `name` set to `value`.

#### Command.removeEnv

```tondo
pub fn Command.removeEnv(self, name: String): Command
```

The command without the environment variable `name`.

#### Command.clearEnv

```tondo
pub fn Command.clearEnv(self): Command
```

The command with no environment variables but those it sets.

#### Command.stdin

```tondo
pub fn Command.stdin(self, input: Input): Command
```

The command reading its standard input from `input`.

#### Command.stdout

```tondo
pub fn Command.stdout(self, output: Output): Command
```

The command writing its standard output to `output`.

#### Command.stderr

```tondo
pub fn Command.stderr(self, output: Output): Command
```

The command writing its standard error to `output`.

#### Command.mergeStderr

```tondo
pub fn Command.mergeStderr(self): Command
```

Sends standard error where standard output goes: `2>&1`.

#### Command.group

```tondo
pub fn Command.group(self): Command
```

The command started in a process group of its own, which `cancel` and
`signal` reach whole: also what it starts, as the test programs that
`go test` builds. The terminal's Ctrl-C then reaches the program alone,
which decides, with `signals`.

#### Command.grace

```tondo
pub fn Command.grace(self, grace: time.Duration): Command
```

The command with time to finish: `cancel`, and cancelling the task that
waits for it, send `Terminate` and `Kill` only after `grace`, as
`docker stop` does; once it ends, output a descendant keeps open is
waited for at most as long.

#### Command.invokedAs

```tondo
pub fn Command.invokedAs(self, name: String): Command
```

The command with `name` as its program's `argv[0]`, which
`env.invokedAs()` reads there.

#### Command.exec

```tondo
pub fn Command.exec(self): Never ! ProcessError
```

The program becomes the command's, with its environment, directory and
redirections to files; it returns only when it cannot.

### Pipeline

#### Pipeline.stdin

```tondo
pub fn Pipeline.stdin(self, input: Input): Pipeline
```

The pipeline's first stage reading its standard input from `input`.

#### Pipeline.stdout

```tondo
pub fn Pipeline.stdout(self, output: Output): Pipeline
```

The pipeline's last stage writing its standard output to `output`.

#### Pipeline.mergeStderr

```tondo
pub fn Pipeline.mergeStderr(self): Pipeline
```

Sends the standard error of every stage where its standard output
goes: `|&`.

#### Pipeline.group

```tondo
pub fn Pipeline.group(self): Pipeline
```

The pipeline started in a process group of its own, as
`Command.group` starts a command.

#### Pipeline.grace

```tondo
pub fn Pipeline.grace(self, grace: time.Duration): Pipeline
```

The pipeline with time to finish, as `Command.grace` gives a command.
