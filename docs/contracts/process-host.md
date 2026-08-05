# Bootstrap `std.process` contract

**Status:** implemented normative M8 contract for the `tondo-vm-hosted`
target.

## Boundary

`Command` and `Pipeline` are language intrinsics. `std.process` is the only
bootstrap standard module that can construct or execute them. `std.bytes` owns
the canonical immutable `Bytes` value; process output exposes that type and
decodes it through the language-level `String(Bytes)` conversion. The module is
present only when the selected target advertises the registered `process`
capability; importing it without that capability is rejected with `E1008`
before HIR construction or runtime execution.

The bootstrap surface is:

```tondo
opaque ExitStatus
opaque ProcessOutput
opaque ProcessError
opaque ProcessExitError
opaque terminal ProcessHandle

fn args(): Array[String]
fn command(program: String, arguments: ...String): Command
// `cmd` is retained only as an internal bootstrap compatibility alias.
fn shell(text: String): Command
fn Command.mergeStderr(self): Command
fn Pipeline.mergeStderr(self): Pipeline

fn Command.start(self): ProcessHandle ! ProcessError
async fn Command.status(self): Array[ExitStatus] ! ProcessError
async fn Command.output(self): ProcessOutput ! ProcessError
async fn Command.run(self): Array[ExitStatus] ! ProcessError
async fn Command.check(self): ProcessOutput ! (ProcessError | ProcessExitError)

fn Pipeline.start(self): ProcessHandle ! ProcessError
async fn Pipeline.status(self): Array[ExitStatus] ! ProcessError
async fn Pipeline.output(self): ProcessOutput ! ProcessError
async fn Pipeline.run(self): Array[ExitStatus] ! ProcessError
async fn Pipeline.check(self): ProcessOutput ! (ProcessError | ProcessExitError)

async fn ProcessHandle.status(self): Array[ExitStatus] ! ProcessError
async fn ProcessHandle.output(self): ProcessOutput ! ProcessError
async fn ProcessHandle.run(self): Array[ExitStatus] ! ProcessError
async fn ProcessHandle.check(self): ProcessOutput ! (ProcessError | ProcessExitError)
async fn ProcessHandle.cancel(self): Array[ExitStatus] ! ProcessError
```

`ProcessOutput` exposes the read-only fields `stdout: Bytes`, `stderr: Bytes`,
`combined: Bytes`, and `statuses: Array[ExitStatus]`. `ExitStatus` exposes
`code: Int?` and `success: Bool`. These are opaque standard-library values at
the host boundary even though their observable field contract is record-like.

`ProcessError` and `ProcessExitError` are distinct nominal
standard-library errors. `ProcessError` represents spawn, pipe, read, and wait
failures. `ProcessExitError` represents a completed `check` for which at least
one stage was unsuccessful and retains the complete `ProcessOutput`.
`std.bytes.Utf8Error` represents strict UTF-8 decoding failure.

Source annotations qualify these source-less bootstrap types through the
module, for example `process.ProcessError`. A `check` result is a normal
discriminated union and supports arms such as
`err(process.ProcessError(_))` and
`err(process.ProcessExitError(_))`.

## Plans and composition

`command` stores the program and each argument separately. It performs no shell
parsing, tokenization, globbing, interpolation, environment expansion, or
execution. Every argument reaches the operating-system process API with the
same sequence of Unicode scalar values received from its Tondo `String`.

The compiler may still accept `process.cmd` while bootstrap fixtures migrate,
but it lowers to the same typed command plan and is not part of the public
stdlib surface.

`shell` is the only shell constructor. It creates one explicit stage using the
platform shell (`/bin/sh -c` on Unix and `cmd.exe /C` on Windows). No other
constructor or operator introduces a shell.

The `|` operator accepts exactly:

```text
Command  | Command  -> Pipeline
Command  | Pipeline -> Pipeline
Pipeline | Command  -> Pipeline
Pipeline | Pipeline -> Pipeline
```

It appends stages left-to-right. Construction is inert and deterministic.
Plans contain only immutable copied configuration and therefore satisfy
`Copy + Discard + Send + Share`; they never own children, descriptors,
streams, tasks, or cleanup obligations.

## Terminal operations

`start` is synchronous only for process creation. It returns immediately after
all stages and their OS pipes have been created, or returns `ProcessError`.
The resulting `ProcessHandle` is affine and terminal. Exactly one of
`status`, `output`, `run`, `check`, or `cancel` must consume it.

The plan operations perform `start` plus the corresponding terminal handle
operation. They are async because they may wait for child exit or stream I/O:

- `status` drains both output streams, waits for every stage, and returns all
  statuses in pipeline order.
- `output` drains both streams, waits, and returns bytes plus every status.
- `run` has the same wait and drain behavior but returns only statuses.
- `check` returns `ProcessOutput` only when every stage is successful;
  otherwise it returns `ProcessExitError`. A stage terminated by the
  platform's broken-pipe condition because a successful downstream stage
  deliberately stopped reading is also satisfactory; its observable
  `ExitStatus.success` remains `false`. Spawn and I/O failures remain
  `ProcessError`.
- `cancel` requests termination of every still-running stage, closes owned
  pipe endpoints, reaps every child, and returns their final statuses.

A non-zero exit is data for `status`, `output`, and `run`. It is never a
panic. Only `check` turns it into the named recoverable error.

Pipeline stderr is captured independently per stage and concatenated in
pipeline order after all readers finish. `ProcessOutput.combined` additionally
records stdout/stderr chunks in the order observed by the host, without
pretending that the operating system supplied a line order. No content-based
reordering is performed.

## Streams, backpressure, and scheduling

`|` connects the previous stage's stdout directly to the next stage's stdin
with an operating-system pipe. No unbounded Tondo buffer is inserted, so the
kernel pipe applies backpressure. Stderr is never merged implicitly.

`Command.mergeStderr()` and `Pipeline.mergeStderr()` are explicit typed
redirections. On a non-final stage they connect both stdout and stderr to the
next stage's stdin (`|&` / `2>&1 |`); on the final stage they expose both streams
through stdout while retaining the same bytes in `combined`. Shell syntax is
not parsed by `command` or by `|`.

The final stdout and every stderr are drained concurrently while children run.
The cooperative Tondo executor never performs a blocking child wait or stream
read on its worker. Host work runs independently and completion is polled
between runnable Tondo tasks; the executor may block for a host completion only
when it has no runnable language task.

`String(Bytes)` uses strict UTF-8 and never performs replacement decoding.

## Cancellation and cleanup

Each running process group has one host-owned cleanup record. Cancellation,
panic unwind, VM failure, and normal host destruction all use the same
idempotent cleanup path:

1. close host-owned pipe endpoints;
2. request termination for every live child;
3. wait for and reap every child;
4. join stream-drain workers;
5. publish at most one terminal result.

On Unix each pipeline stage is placed in a fresh operating-system process
group, so cleanup also terminates descendants created by an explicit shell
stage before reaping the group leader.

Dropping a live `ProcessHandle` is rejected statically by terminal ownership.
The host cleanup record remains a defensive runtime boundary and guarantees
that an aborted VM run cannot leave children or zombies behind.

## CLI arguments

For `tondo run script.to -- arg...`, `process.args()` returns only the values
after `--`, in order and converted from the platform's UTF-8 command-line
representation. `fmt` and `check` reject program arguments. Invalid non-UTF-8
arguments are usage errors at the CLI boundary rather than lossy strings.
