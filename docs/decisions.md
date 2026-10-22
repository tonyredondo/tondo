# Design decisions

Short records of decisions that shape the implementation. Each entry states
the decision and why; the specification remains the authority on language
behavior.

## 2026-10-21 — Native-first restart

The implementation starts over from the specification. The previous one was
built around a bytecode VM, with native compilation as a later addition, and
did not produce native executables. This one compiles to native code from the
first milestone and grows in vertical slices.

## 2026-10-21 — Cranelift and `cc`

Code generation uses Cranelift (`cranelift-object`) for fast compilation and a
pure-Rust toolchain. The system C compiler links objects with the runtime. A
self-contained linker is future work (H12).

## 2026-10-21 — `run` compiles, like `go run`

There is no interpreter: `tondo run` builds a native executable, caches it by a
fingerprint of the compiler and the source, and executes it. One backend means
one semantics, so what runs is what ships.

## 2026-10-21 — Runtime in Rust, embedded in the compiler

The runtime is a dependency-free Rust crate compiled as a static library by the
compiler's build script and embedded with `include_bytes!`, so a single `tondo`
binary is enough to build programs. `unsafe` is allowed in the runtime crate:
it owns allocation, reference counts and the private ABI with generated code.
Executables link with `--gc-sections` and without debug info from the standard
library (a hello world is about 430 KB).

## 2026-10-21 — Reference counting with explicit MIR operations

Managed values (strings for now) are reference counted. Lowering inserts
`Retain`/`Release` into MIR: parameters are borrowed, call results are owned
temporaries released at the end of their statement, bindings release at scope
exit and on every early exit. Each conditionally evaluated region has its own
temporary frame, so cleanup only touches initialized values. Counts are atomic
because `spawn thread` will share values. Copy-on-write collections (H1) reuse
the same counts. Only types that can reach `Ref[T]` can form cycles, so the
cycle collector, when it comes, only needs to track those.

## 2026-10-21 — Stackful tasks for async

Tasks will run on their own small stacks (H5). Tondo's implicit waiting and
inferred `suspends` then need no state-machine transformation, loans stay valid
across suspension because stacks do not move, and stack traces stay ordinary.
The costs are per-task stack reservations and a small context switch per
architecture.

## 2026-10-21 — `ok`, `err` and `some` are contextual

They construct `Result`/`Option` only when followed by `(`, so `ok` and `err`
remain usable as variable names, which programmers and LLMs coming from Go use
constantly. `none` stays reserved because a binding named `none` in a pattern
would silently match everything. `Some`/`None`/`Ok`/`Err` produce errors that
name the canonical form. See language spec 5.5.

## 2026-10-21 — Aggregates live in memory

Tuples and records have a C-like layout in declaration order. Locals of
aggregate type get stack slots; aggregate parameters are passed as pointers to
the caller's memory (parameters are immutable and borrowed), and aggregate
results are written through a pointer the caller passes first. Newtypes share
the representation of their value. MIR addresses fields through places
(`local` plus a field path); retain and release recurse through the counted
components of a type. This keeps code generation direct; scalar replacement of
small aggregates is a possible later optimization.

## 2026-10-21 — Enums, boxes and glue functions

Enums store a tag (one byte up to 256 variants) followed by a payload area
shared by all variants; payload-free enums are plain scalars. Recursive types
need indirection (9.10): every cycle of inline storage passes through a
nominal type, so the edge into a nominal type of the same cycle is stored as
a box, a reference-counted heap cell. Copies share boxes and a write through a
box first makes it unique (copy-on-write), which preserves value semantics.
Retain and release of structs are emitted inline and stop at boxes and enums,
which use generated per-type glue functions; the runtime calls the release
glue when a box dies. Productivity is a least fixed point, so only recursive
types without any finite value are rejected (E1107).

## 2026-10-21 — Exhaustiveness with witnesses

`match` exhaustiveness and unreachable arms use Maranget's usefulness
algorithm. Errors name a value that is not covered, such as `Shape.Point` or
`(false, true)`, which gives the programmer (or an LLM) the exact arm to add.

## 2026-10-21 — Displaying floats

A displayed float is the shortest decimal that reads back as the same value
and is always distinguishable from an integer: `1.0`, `0.1`, `1e-7`, `1e21`,
`NaN`, `inf`, `-inf`. The standard library specification (10.6) now fixes this
together with the text of displayed collections.

## 2026-10-21 — Narrow arguments to the runtime are extended

Calls into the Rust runtime mark 8- and 16-bit integer parameters as
zero-extended. LLVM, which compiles the runtime, assumes the caller extended
them to 32 bits; without it a `Bool` with garbage high bits printed `fals`.

## 2026-10-21 — One vocabulary for collections

Collection methods are intrinsic and need no import. Each operation has one
name everywhere: `length` and `isEmpty` on every collection, `push`/`pop` at
the end of `Array` and `Deque`, `get` for positions (`get(0)` and `get(-1)`
observe the ends), `clear` on all of them. `push` cannot fail: running out of
memory is resource exhaustion, not a recoverable error. Queues use `Deque`,
whose `pushFront`/`popFront` are amortized constant time; arrays only offer
`insert(at, value)` and `removeAt(at)`. Names from other languages (`append`,
`size`, `peek`, `enqueue`, `put`, `containsKey`...) are rejected with the
canonical replacement in the message. See standard library spec 10.2.3.

A deque shares the array's representation: the header records where the
elements start, and the deque keeps them in the middle of storage at least
twice their size, so both ends have room. Arrays always start at 0, so their
generated code never reads that field.

## 2026-10-21 — Lifted arithmetic in the runtime

`+ - * / %` over arrays (10.6) call one runtime function that receives both
operands, their array depths and the number type. It checks the whole shape
first (`P0006`) and then computes in order with the checked scalar rules, so
panics match the scalar operators. `a += b` builds the new array and then
stores it, which gives the strong guarantee for free. A slice assignment of a
different length also panics with `P0006`: 10.5 requires equal lengths but
does not name the panic.

## 2026-10-21 — Unions are anonymous enums

A union type is normalized when it is built: nested unions are flattened,
`Never` and duplicates disappear, and members are sorted by name, so
`A | B` and `B | A` are one type with one layout. Its value is an enum with
one variant per member, which reuses the enum layout, boxes for recursive
members, glue functions and Maranget's exhaustiveness (a missing member is
reported as `Int(_)`). A value is injected only when its type is exactly a
member; an unsuffixed literal takes the only integer or float member, and
`none`/`some`/`ok`/`err` the only option or result member. `?` widens an
error into the function's error union, re-tagging when the source is itself
a smaller union. In patterns, `Member(p)` tests a member and binds the whole
value, while record, variant, option and literal patterns select the member
their own type belongs to; a bare type name is rejected because it would
bind everything.

## 2026-10-21 — Printing cannot fail

`console.print` and `console.println` return `Unit`: they only append to the
runtime's output buffer, so the `!ConsoleError` they used to return could
never report anything and only forced `_ =` on every line. The runtime
delivers the buffer by itself (when it fills, at every line on a terminal,
and at exit), so programs never flush. `console.flush()` returns delivery
failures to programs that want them; a failure nobody observed is reported
on stderr at exit with status 1, and a closed pipe ends the process silently
through `SIGPIPE`, like other command-line tools. See standard library spec
19.1.

## 2026-10-21 — Generics: checked once, monomorphized

Generic functions are checked once with their type parameters as opaque
types; what generic code may do with a parameter comes only from its bounds
(12.5). Type arguments are inferred at each call by unification from the
explicit arguments, the values (literals and other context-dependent
values last) and the expected type. A monomorphization pass then copies
each function reachable from `main` once per combination of type arguments,
so lowering and code generation only see concrete types. An instance of a
generic record or enum, such as `Pair[Int, String]`, is an ordinary nominal
type whose fields are the template's with the arguments substituted; it
reuses every layout and glue path. Instantiations that keep growing, such
as polymorphic recursion, are rejected instead of looping.

## 2026-10-21 — Traits resolve at monomorphization

A call to a trait method, through a bound (`value.summarize()` with
`T: Summary`) or qualified (`Summary.summarize(value)`), is checked against
the trait's signature with `Self` as a type argument. Monomorphization
replaces it with the implementation for the concrete `Self`, or with the
trait's default body instantiated for it; coherence leaves at most one
candidate. Interpolating a user type calls its `Display.display`; built-in
values keep their intrinsic `Display`, and the user implementations needed
by collections being displayed are recorded for code generation.
Implementations whose bounds could require themselves forever are rejected
statically by the size-change check of 12.6 (`E1112`).

## 2026-10-21 — Closures are records of captures

Each closure expression has its own concrete type: an anonymous record of
the bindings it captures, generic over the enclosing function's type
parameters. Its body is a function whose first parameter borrows the
closure, so a `var` closure keeps changes to its captures between calls,
while calling it through anything else works on a copy (11.8). Bounds such
as `F: Call[fn(Int): Int]` pass closures to generic code with direct calls
after monomorphization. A `fn(...)` value is a reference-counted object
holding code and captures: named functions are static objects with a thunk,
and a closure that never changes its captures is boxed when coerced; a
changing closure is rejected with `E1108`.

## 2026-10-21 — Opaque results are revealed by monomorphization

An `impl A + B` result (12.8) is an opaque type over the function's type
parameters whose bounds are those of a hidden parameter, so callers use it
like a bounded type parameter. The checker infers its witness from the
body, requires one concrete type on every path, checks it against the
published bounds and requires `Discard`. Monomorphization replaces the
opaque type with the witness instantiated for each call.

## 2026-10-21 — `defer` is a closure the runtime can also run

A `defer` creates a closure when it registers: the block form captures
values then, and the call form evaluates its operands then (13.7). It is
stored in a hidden local and called on every exit of its scope in LIFO
order. Registration also pushes the closure's body and address onto a
runtime cleanup stack, popped before the normal call. A panic runs the
remaining entries before terminating: since a panic ends the process,
frames are still alive and the native stack never unwinds. A panic inside
a cleanup is reported as suppressed and the other cleanups still run.

## 2026-10-21 — Ownership is checked on generic code before monomorphization

Every concrete value of 0.1 is `Copy`, so moves and obligations (8.10) only
concern type parameters declared `~Copy` and the composites that hold them. The checker analyses each function's HIR once,
before monomorphization, following its structured control flow: branches
merge "moved" into "moved on some paths", and a loop body is checked again
with what its previous iteration moved. A use of a moved binding is
`E1401`; moving a part, copying a non-`Copy` element out of a place or
moving out of a loan is `E1406`; overwriting a value that cannot be
dropped is `E1408`; and dropping one at a scope exit, with `_ =`, a `_`
pattern or `clear()` is `E1409`. `E1404` is left for the terminal types
that come with native handles. Closure bodies are not analysed yet.

## 2026-10-21 — Type parameters are `Copy` unless they say `~Copy`

Requiring `T: Copy` or `T: Discard` wherever generic code reads, indexes
or drops a value put a bound on almost every generic function. Type
parameters of functions, methods and impls, and opaque results, now assume
`Copy`, as Swift assumes `Copyable`; `~Copy` drops the assumption for code
that also accepts affine values, and `Discard` can be added back. A type's
parameters and a trait's `Self` and parameters assume nothing, so data can
hold anything and each function says what it needs. One relaxation keeps
the syntax small: `~Copy + Discard` covers values that may be dropped but
not copied.

## 2026-10-21 — A region loan is a copy that goes back in place

`mut values[start:end:step]` (16.4) lends the callee a temporary array with
the selected elements and assigns them back to the same region after the
call. The loan is exclusive and `mut` keeps the length, so nothing can
observe the difference from a view into the original storage, and arrays
keep one representation everywhere. Bounds and indices are evaluated once,
in the order written; then the call checks that its exclusive loans of one
array share no element (`P0004`), and lending a whole place together with
part of it is rejected statically (`E1403`). `ref values[start:end]` passes
the slice as a read-only value.

## 2026-10-21 — Borrowed `for` bindings point into the collection

A `for` whose pattern has `mut` or `var` bindings (13.3) walks the
positions of the array or map; each iteration binds every binding of the
pattern to the address of its part of the element, after giving the
collection storage of its own, so writes reach the collection and never a
snapshot. The collection cannot be used directly or resized while the loop
runs (`E1403`), which keeps those addresses valid. With only `ref`
bindings the loop reads the elements like any other `for`, and the
collection cannot change inside it.

## 2026-10-21 — Loan modes are part of function types

`fn(mut Array[Float], Float)` and `fn(Array[Float], Float)` are different
types (16.3): a call through a function value lends `mut` and `var`
arguments by address, as a direct call does, and a closure declares the
mode of each parameter or takes it from the expected type. Terminal resources and their unwind actions move to H6, where
the first types that need them (files, processes) arrive.

## 2026-10-21 — Modules are linked by renaming

A project's directories are its modules (6.1). Before checking, the module
linker resolves every import and renames each declaration of a module other
than the root after its module path, and each path that reaches it: `User`
in `app.models` becomes `models.User`, which is also how code that imports
`app.models` writes it. The checker then works on one namespace and shows
types as they are written. The linker reports unknown modules and
declarations (`E1008`, `E1001`), private declarations (`E1501`) and import
cycles with their path (`E1006`); the checker knows the module of each
declaration and of the code it checks, for `priv` fields, private
operations, public APIs that expose private types (`E1503`) and coherence.
Imports of `std` modules are shared by the files of a program for now.

## 2026-10-21 — JSON diagnostics without structured types yet

`tondo check --json` prints the diagnostics in the form of 22.3, ordered as
22.6, with SHA-256 ids. `source_id` is `pkg:<name>` for a project and
`root:<file>` for a single file. `expected`, `actual`, `related` and `fixes`
are `null` or empty until the checker records them as data; the messages
already state the fix in words.

## 2026-10-21 — The formatter prints the syntax tree back

`tondo fmt` builds the document of 21.2 from the syntax tree rather than
from a lossless tree: literals are copied from the source, the parser
records which expressions were written in parentheses, and the lexer
records comments, which return before or after the unit they touch.
Ranges stay tight (`0..count`), as every example of the specification
writes them, and a string of several lines stays on the line of its `=`.
A block that produces a value and holds only an expression (a closure
body, a block used as a value, the branches of an `if` with `else`) stays
on one line when it fits, so short lambdas and conditional values read as
in Kotlin or Swift; function bodies and loops are always blocks.

## 2026-10-21 — Dependencies come from paths, git and archives

There is no registry, so a dependency names where it lives: a directory, a
git repository at a tag, branch or commit, or an archive URL with an
optional SHA-256. Fetching uses the system's `git`, `curl` and `tar`, as
linking uses `cc`, into a cache that is never modified in place. `tondo.lock`
is written by the toolchain, Cargo-style: it pins the commit or content of
every fetched dependency of the graph, builds use the pins without the
network, and `tondo update` resolves again. Each package's modules are named
after the package, and a package imports its own modules and the aliases it
declares. The toolchain specification was rewritten around this; targets,
capabilities, generators, compiled interfaces and the internal JSON
manifests of the earlier draft were removed.

## 2026-10-21 — Concurrency without colors

Functions are not colored: any call may wait, and waiting stops the task,
not the thread. There is no `await`, `suspends`, `selectable`, `@sync`,
`scope` or `spawn thread`. `spawn call()` returns a `Join[T]` whose `wait()`
gives the call's result, with an optional `timeout:` that cancels the task;
an unconsumed `Join` is waited when its block ends, so no task outlives its
owner and nobody writes anything to ensure it. `Group[T]` covers dynamic sets
(first error cancels the rest), channels carry streams (`channel[T]()`
unbounded, `channel[T](n)` bounded, closed when the last moved `Sender`
disappears), `select` waits for the first of `receive`, `send`, `wait`,
`next` and `sleep`, and `std.sync` holds shared state. `Group`, `oneshot`'s
role, unbounded channels and timeouts were kept or added after review: the
aim is a middle ground, not channels for everything. `oneshot` and
`AsyncIterator` were dropped: `Join` covers a future result, and any
`Iterator` may wait in `next`.

## 2026-10-21 — One task queue for all cores

`main` runs as the first task on the process's main thread, and the first
`spawn` starts one worker thread per core. Workers take tasks from a single
queue; a task that waits saves its registers on its own stack and switches
back to its worker, so compiled code knows nothing of scheduling. Task stacks
are 8 MiB, mapped lazily with a guard region and reused across tasks. A
`Join` counts only the program's references and the last one waits for the
task, which is how an unconsumed `Join` waits at the end of its block. When
every worker is idle and nothing can run, the program panics with `P0013`.
A single queue keeps the scheduler small; work stealing can replace it when
measurements show contention.

## 2026-10-21 — A move gives the references away

Values are reference-counted and calls borrow their arguments, so moving a
value used to be a copy, with the source released at the end of its scope.
That is invisible for ordinary values, but not for a `Sender`, whose channel
closes when the last one disappears, or for a `Join`. The ownership analysis
now records where a binding that is not `Copy` moves; lowering hands its
references to a temporary there and zeroes the binding, so the end of the
scope releases nothing. Moving a sender into `spawn` or into a call therefore
closes the channel when that task or call is done with it. Releases accept a
zeroed value, which needs no drop flags.

## 2026-10-21 — Waits are claimed, then notified

`select` and timeouts wait for the first of several operations, so a wait
is an object of its own: the task registers it with each join, group,
channel or timer, and whoever completes one of those operations first
claims it with a compare-and-swap, under that object's lock, and then
notifies the task. A notification that arrives before the task has
switched out is kept as a token, so no lock is held across a switch.
`select` locks all of its objects in address order while it checks which
operations are ready and registers itself, as Go does, which makes the
check and the registration one step. Timers live in a heap that idle
workers sleep on; a wait that ends another way leaves its timer behind, and
those are purged once they are most of the heap.

## 2026-10-21 — Cancellation unwinds through ordinary returns

A cancelled task must run its `defer`s and release its values, but a panic
ends the process and so only runs cleanups. Cancellation instead reuses the
paths `return` already has: after every call that may wait, compiled code
asks the runtime whether the task is cancelled and, if so, returns at once
through the function's cleanup, with zeros its caller never uses. Which
calls may wait comes from a whole-program pass over the lowered functions
(any waiting operation, call through a function value, or call of a
function that may wait), so code that never waits pays nothing. Waiting
operations end at once in a cancelled task and leave zeros where a result
would go; zeroed values release safely. Releasing a `Join` or a `Group`
from a cancelled task cancels those tasks too and waits for them.

## 2026-10-21 — `std.sync` hands its state to closures

`std.sync` keeps the classic toolbox — `Mutex`, `RwLock`, `Semaphore`,
`Once`, `Barrier`, `Atomic` and five shared collections — because shared
state should not have to go through channels. Tondo has no references to
return, so a lock never hands out a guard to its value: `update` and `read`
run a closure with the state lent as `var` or by value, and the lock is
released when the closure returns, whatever happens. `updateWhen` replaces
condition variables: it waits until a predicate holds, checked again after
each change, so there is no `notify` to forget. Only `Permit` is a value
of its own, so that moving it into a task bounds concurrency. Atomics have
no memory orderings, and a `for` over a shared collection walks a
snapshot. Each value is a lock plus an ordinary copy-on-write value, so
snapshots cost O(1); the checker turns every operation into a block that
takes the lock and runs the ordinary operation on a loan of the state.
Taking a lock the task already holds panics with `P0014` instead of
hanging.

## 2026-10-21 — Shared collections are built for contention

A collection behind one lock is something any program can write; the
standard library's are worth having only if tasks rarely wait for each
other. `sync.Map` and `sync.Set` follow .NET's `ConcurrentDictionary`:
readers walk the buckets without locks, a writer locks one of several
stripes per core, and growing locks them all and copies the nodes.
`sync.Queue` is a lock-free queue of segments in which producers and
consumers never block each other, `sync.Stack` is a Treiber stack whose
nodes record the depth, so `length()` is one read, and `sync.Array` locks
stripes of slots. Without a garbage collector, memory a reader may still
see is freed through epochs. Two semantics follow from this, as in .NET,
Java and Go: shared maps and sets have no order, and their `length()` sums
per-stripe counters without locking, so it is exact only when no writes
run concurrently. Atomics of integers, `Bool`, `Char` and `Byte` use
hardware instructions. The task lock was also reworked: one compare-and-
swap when free, a short spin, then parking, and releasing lets a running
task take it ahead of the woken one, which avoids lock convoys.

## 2026-10-22 — Locks starve nobody

Letting a running task take a released lock ahead of the woken one avoids
convoys, but it can leave one task losing indefinitely, and with readers
always present a writer of an `RwLock` might never get in. As in Go's
`sync.Mutex`, a woken task that keeps losing for more than a millisecond
turns the lock to starving: nobody may take it ahead of the queue, and each
release hands it to the first waiter, or to every reader in a row. The lock
returns to normal when the queue empties. With eight readers that never
stop, a writer now waits 4 ms instead of up to a second.

## 2026-10-22 — Files without ceremony

`std.fs` keeps to what programs use: whole-file reads and writes, a
directory listing and a `File` for reading line by line or in chunks.
Paths are plain `String`s and `std.path` only works on their text, so
there is no path type to convert to and from. Binary data is
`Array[Byte]`, with `text.bytes()` and `String.fromBytes` between it and
text, instead of a separate bytes type. A `File` closes itself at the end
of its scope; `close()` exists to see the errors of closing, and the
`E1404` terminal resources stay for values whose ending must not be
silent. Errors are one `FsError` enum, which displays as its variant in
words, like every standard error. Each operation runs on a pool of host
threads while the task waits, so a slow disk never holds a worker, and
whole-file writes go through a temporary file and a rename, so a reader
never sees half a file. There is no capability for the filesystem: a
program that imports `std.fs` uses it.

## 2026-10-22 — Parameters with default values

An optional decision such as deleting a directory with its contents is one
parameter, `fs.remove(path, recursive: true)`, rather than a second
function or an options record. Defaults are constants (literals, `none`,
`[]`, or variants, tuples and records of them), evaluated at each call that
omits the argument, so they never depend on other parameters or on state.
Loans, variadic parameters and trait methods have none, and a `fn(...)`
value forgets them. LLM authors write defaults in Python, C# and Kotlin;
the error for a missing argument now names its type.

## 2026-10-22 — No terminal obligations; resources clean up on exit

The spec had types whose values could not be dropped: every normal exit
from a scope had to call a terminal operation such as `wait()` or
`close()`, or register a `defer` for it, or fail with `E1404`. Supporting
that took the `Discard` capability and its bounds, rules against
overwriting, ignoring with `..` or `clear()`-ing such values, unwind
actions, armed and disarmed cleanup entries, `defer` guards and confirmed
transfers in every call. All of it is gone. Every value can be dropped,
and a resource's type defines what dropping it does, as files and joins
already did: a file flushes and closes, a join waits, a process handle
closes the child's input and waits, and a panic cancels instead.
`close()`, `wait()` and `cancel()` remain to see errors. A deferred call
now takes its arguments when it is registered.

The opposite case, work nobody should wait for, is explicit:
`job.detach()` lets a task run on, and when `main` returns the runtime
cancels the detached tasks still running and waits for their cleanup.

## 2026-10-22 — One stream type behind files, console and memory

`std.io` has a `Writer`, a `Reader` and a memory `Buffer`. Streams are
shared handles, as in Go and .NET: copies use the same stream, every
operation is atomic, and the stream closes with its last copy. That lets
the console hold one: `console.setOutput(buffer.writer())` captures
everything printed, from every task, until `console.setOutput(previous)`,
and a file or a process pipe can stand in the same way. In the runtime a
`File`, a `Writer` and a `Reader` are the same object, so `file.writer()`
converts nothing. Work that may block runs on host threads, with no limit
for reads that can wait indefinitely such as a terminal, but an operation
the bytes already read ahead, or the room left to gather writes, can
satisfy runs in place: reading stdin line by line is faster than Python.
`ConsoleError` is gone; console and streams fail with `io.IoError`
(`Closed`, `InvalidData`, `Host`), and `console.readLine()` replaces
invalid UTF-8 rather than failing, since what a person types is text.

## 2026-10-22 — The program owns a copy of its environment

`env.set`, `env.setAll` and `env.remove` change the environment of the
whole program, and processes it starts afterwards inherit it, as in every
mainstream language. The runtime does not call the host's `setenv`, which
is unsafe while other threads read the environment, and our programs
always have several; it keeps a table of its own that it passes to child
processes. Native code reached through FFI would not see the changes, so
`env.syncNative()` copies them to the host explicitly. `env.args()` leaves
out the program's name, and invalid UTF-8 in arguments and variables
becomes U+FFFD; an invalid variable name is a programming error and
panics (`P0015`).

## 2026-10-22 — Time without failure modes

`time.now()` reads a monotonic clock and cannot fail; there is no clock
domain to mismatch, no `ClockError` and no `DurationError`. A `Duration`
overflows like the `Int` it holds: with the panic `P0005`. Durations
display as Go writes them (`1h2m3.5s`, `250ms`), which LLM authors and
people already read. For the wall clock there is `time.utcNow()`, a
`DateTime` record in UTC that displays in RFC 3339, and `time.unixTime()`;
time zones, local time and calendar arithmetic wait for a later version.
Integer arithmetic on a newtype now follows the sign of the integer it
holds; it used to check `time.Instant` subtraction as unsigned.

## 2026-10-22 — Processes with pipes and typed redirections

`std.process` keeps the spec's plans and adds what a shell offers without
parsing shell syntax. `Command` and `Pipeline` are immutable shared plans
composed with `|`; `stdin`, `stdout` and `stderr` take an `Input` or
`Output` enum for `<`, `>`, `>>`, `2>&1`, `/dev/null`, a fixed text, a
stream of `std.io` or a pipe the program reads and writes through the
`Process`. `run`, `status`, `output`, `check` and `start` differ only in
their default streams, and an explicit redirection always wins. A non-zero
exit is data except for `check`, which returns `Failed` with the status
and the whole output; a pipeline succeeds only when every stage does,
except a stage ended by a closed pipe before a later one succeeded, so
`yes | head -1` succeeds. A child that inherits the console writes to the
program's replaced console stream when there is one, so capturing the
console captures children too. Waiting runs on host threads and a
cancelled task kills its children; dropping a `Process` closes its input
and waits, a panic kills them, and `detach()` lets them outlive the
program.

## 2026-10-22 — std.json is written in Tondo, and every type encodes

`std.json` is the first standard module written in Tondo itself: the
compiler carries its source and links it as a module of a package `std`
when a program imports it. Rust provides only what Tondo cannot do as
fast, a tokenizer and an emitter, reached through `std.internal.json`.
The surface follows the spec's three routes. The typed one, `encode` and
`decode`, works for any record, enum and newtype without `derive`: the
compiler writes the `Encode` and `Decode` implementations as Tondo source
in the type's module, for every type whose parts can be encoded, and a
hand-written `impl` replaces them. `@name("wire")` and `@ignore` on fields
are the only knobs. The dynamic route is `json.Value`, which a program
navigates as `document["users"][0]["name"].asText()`, missing parts
reading as `Null`. The event route is a `Reader` and a `Writer`, which the
other two are built on. `Number` keeps numbers exact, `Raw` keeps a value
unparsed, and `canonical: true` writes RFC 8785. The traits live in
`std.json` until a second format needs `std.serialization`.

Two language rules the spec already had now work, which JSON code relies
on: a fallible function may end with, or `return`, a whole `T ! E`, which
propagates like `?`; and `call(...)?` infers the call's type arguments
from the type its result is expected to have, so
`let users: Array[User] = json.decode(text)?` needs no `[Array[User]]`.

## 2026-10-22 — Unused values are dropped, except results and options

Requiring `_ =` for every unused value made `cmd.run()?`, `cache.insert(...)`
or a closure whose last call returns something ceremony. A value nobody
uses is now dropped, as in Go, Kotlin or Swift; only a `Result` or an
`Option` still has to be used, since it may hold a failure or an absence,
as Rust's `must_use` does for its `Result`. The same holds for the final
expression of a block whose type is `Unit`, which Tondo cannot mark as a
statement with a semicolon. `T ! E` now displays as `ok(value)` or
`err(error)`, like `T?`.
