# std.process

Running other programs, and what this one is to the system. A
[`Command`](#command) is a program and its arguments, each on its own, so
nothing is split at spaces, expanded or quoted, and a file name with a
space or a `$` in it is only a file name. Commands join into pipelines with
`|`, with no shell in between, and running one waits without holding a
thread.

- [Running a program](#running-a-program)
- [Pipelines, input and where it runs](#pipelines-input-and-where-it-runs)
- [What to reach for](#what-to-reach-for)

## Running a program

```tondo
import std.console
import std.process

fn main(): !process.ProcessError {
    let listing = process.command("printf", "%s\n", "tea", "a file with spaces", "$HOME").output()?
    console.println("{listing.text()?.lines()}")

    // `status` gives how it ended; `check` fails when it did not end well.
    let found = process.command("test", "-d", "/").status()?
    let missing = process.command("test", "-d", "/no/such/place").status()?
    console.println("{found.success} {missing.success} {missing.code}")
    match process.command("false").check() {
        ok(_) => console.println("it worked")
        err(error) => console.println("{error}")
    }
    match process.command("no-such-program").status() {
        ok(_) => console.println("it ran")
        err(_) => console.println("there is no such program")
    }
}
```

```text output
[tea, a file with spaces, $HOME]
true false 1
exited with code 1
there is no such program
```

| | |
| --- | --- |
| [`run`](#run) | with this program's input and output: what the user sees |
| [`status`](#status) | without input, discarding output: only how it ended |
| [`output`](#output) | captures what it printed |
| [`check`](#check) | as `output`, failing when the program failed |
| [`start`](#start) | starts it and gives a [`Process`](#process) to write to, read from, wait for or stop |

## Pipelines, input and where it runs

```tondo
import std.console
import std.process

fn main(): !process.ProcessError {
    // `sort | uniq -c`, with no shell: each stage is a program and its arguments.
    let words = process.command("printf", "%s\n", "tea", "cup", "tea", "pot", "tea")
    let counted = (words | process.command("sort") | process.command("uniq", "-c")).output()?
    console.println("{counted.text()?.lines().map((line) { line.trim() })}")

    let here = process.command("pwd").directory("/").env("LANG", "C").output()?
    console.println(here.text()?.trim())
}
```

```text output
[1 cup, 1 pot, 3 tea]
/
```

- [`Command.directory`](#commanddirectory), [`Command.env`](#commandenv)
  and [`Command.stdin`](#commandstdin) say where it runs, with what
  environment and what input.
- Cancelling the task that waits for a command, as a `within` does, ends
  the command. [`Command.grace`](#commandgrace) asks it to end first and
  stops it only after a while, as `docker stop` does, and
  [`Command.group`](#commandgroup) makes that reach everything it started.
- [`shell`](#shell) runs a line through the system's shell, for when that
  is what is wanted.

## What to reach for

| To | Use |
| --- | --- |
| Run a program | [`command`](#command), then [`run`](#run), [`status`](#status), [`output`](#output) or [`check`](#check) |
| Join programs | `first \| second`, a [`Pipeline`](#pipeline) |
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

- [Functions](#functions): [`command`](#command), [`shell`](#shell), [`exit`](#exit), [`signals`](#signals), [`run`](#run), [`status`](#status), [`output`](#output), [`check`](#check), [`start`](#start), [`which`](#which), [`signal`](#signal), [`splitWords`](#splitwords), [`quoted`](#quoted)
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
pub fn run(self): ExitStatus ! ProcessError
```

On a Command or a Pipeline: with the program's streams, waits.

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
_ = process.command("make").stderr(process.Output.File("build.log")).run()?
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
