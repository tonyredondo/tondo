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

## 2026-10-22 — JSON streams in constant memory

`json.readerFrom` pulls 64 KB at a time from an `io.Reader` as the parser
needs input and lets consumed input go, keeping line and column counts for
what it dropped; `json.writerTo` hands output to an `io.Writer` every
64 KB. `decodeFrom` and `encodeTo` are built on them, so reading a 51 MB
document event by event peaks at the memory of an empty program. Only
canonical output buffers, since it sorts keys at the end.

## 2026-10-22 — unsafe as the spec writes it, and the flag on function types

H7 implements 16.12 as written: `unsafe fn`, `unsafe { ... }` blocks,
`unsafe (...) { ... }` closures and `unsafe fn(...)` types, and the six raw
operations of `Pointer[T]` (`read`, `write`, `offset`, `cast[U]`,
`address`, `UInt64.toPointer[T]`), each of them `E1701` outside a region.
Being `unsafe` is part of a function's type, so an `unsafe fn` cannot
become a safe `fn(...)` value and a call through one still needs a
region; a closure that captures a pointer must be `unsafe` (`E1702`), as
must an implementation of an `unsafe` trait method. A write retains a
counted value it copies and a read retains what it copies out, so raw
memory holds references like any owner. With no FFI yet, nothing gives a
program real memory, so `std.internal.memory` allocates zeroed blocks for
the tests of this milestone.

## 2026-10-22 — Calling C: extern declarations, std.ffi, implicit widening

A program calls C with `extern fn` and lays out C structs with `extern
type` (16.16). No ABI string: the C calling convention is the only one, and
`@symbol` names a symbol that differs from the Tondo name. Every call needs
`unsafe`. What crosses is closed: numbers, `Bool`, `Byte`, pointers,
`extern fn(...)`, `extern type`s and `ffi.CString`; a `String` passes where
a `CString` is taken as a copy freed after the call (`P0016` for a zero byte
inside), and arguments of C's `...` get C's promotions. `T?` of an address
is one address, zero for `none`, as C's null pointer, which makes optional
pointers cross without conversion.

`extern type` follows C's layout, with `@packed`, `@align(n)` and
`@count(n)` arrays stored in place; without a body it is opaque. Structs
pass by value with System V's classification: up to two eightbytes in
integer or vector registers, otherwise in memory; `...` goes through a
runtime trampoline that sets `al`, which Cranelift cannot.

A named function or a closure without captures becomes an `extern fn(...)`
through a generated adapter; context travels through `ffi.context`. A
callback on the caller's thread runs in its task and may wait: with C
frames on its stack the task waits on that thread instead of migrating. A
callback from a thread of C runs as a new task while the thread waits.

Instead of a `@blocking` annotation, which an author can forget, workers
mark the time they spend in C and a monitor starts another worker when one
stays in the same call past a millisecond while tasks wait to run, as Go
does; it retires when the call returns.

With it, numbers widen implicitly language-wide when no value can be lost
(`Int32` to `Int`, `Float32` to `Float`), at coercion sites and between
operands; anything that may not fit stays a compile error that gives the
exact checked conversion, so no conversion fails silently or panics at
run time.

## 2026-10-22 — Deadlocks with C: count the threads of C

Whether every task waiting is a deadlock depends on whether C will call
back, which the runtime cannot see; Go stops detecting deadlocks once cgo
callbacks exist. Tondo reports `P0013` exactly when nothing of C can act:
every worker is idle or waits inside a function C called, no worker runs C,
and the process has no thread the runtime did not start (counted from
`/proc/self/task` or `task_threads`, against the runtime's own count, minus
threads of C waiting for a callback's task). Otherwise idle workers look
again every 100 ms. This also removed a false `P0013` for programs waiting
on a callback from a C timer or a library's thread.

## 2026-10-22 — arm64 and macOS before H8

Running on Apple silicon moved ahead of the testing milestone. The C
calling convention became one plan per call in `codegen/cabi.rs`, shared by
calls, signatures and callback adapters: System V on x86-64, AAPCS64 on
arm64 (structs of one to four floats of one type in vector registers,
others up to 16 bytes in integer registers, larger ones by the address of a
copy since Cranelift has no stack struct arguments there, and a struct
that no longer fits on the stack with its class of registers used up), and
Apple's variant, which passes `...` on the stack in 8-byte slots: filler
arguments use up the registers, and integers widen to 64 bits because
Cranelift packs narrower ones. Narrow integers cross as extended `int`s on
every target, since Cranelift does not extend them for Apple. Only x86-64
needs the runtime trampoline for `...`. CI runs on Linux x86-64, Linux
arm64 and macOS arm64; tests can be limited to a platform.

## 2026-10-22 — Constants, static and shared libraries, `tondo bind`

`const` (6.5) is implemented: a constant is checked once, folded when it
is integer arithmetic so that a would-be panic is `E1903`, and inlined
where used; cycles are `E1902` and other runtime values `E1901`.

`[native] libraries` takes a name (dynamic, `-lname`), `static:name` (the
archive `cc -print-file-name` finds), or a file: `.a` links statically,
`.so`/`.dylib` dynamically with its directory as the run-time path.

`tondo bind` reads headers through `cc -E -dD`, so macros and platform
`#if`s are the compiler's, and parses C declarations itself instead of
depending on libclang. It binds the given headers and the headers of their
directory they include directly (curl's `easy.h`, zlib's `zconf.h`, but not
`unistd.h` from `/usr/include`). Pointers are optional, since C allows
`NULL`; structs with bit fields become their bytes with the size GCC and
Clang give them; what cannot be bound is listed in the module. A
test builds a C library as an archive and as a shared library, binds its
header and runs a program against each on every CI platform.

## 2026-10-22 — macOS on Intel, musl, and what `tondo bind` could not bind

CI also runs on macOS x86-64 and, in Alpine containers started from the
host, on musl for x86-64 and arm64. musl's Rust toolchain links its own
`libunwind`, which Alpine's C compiler lacks, so the compiler embeds that
archive and links it in place of `-lunwind`. Cross-compilation with
`--target` waits for the self-contained linker (H12), since linking for
another platform needs its C library.

C unions are `@union extern type`s: fields share offset 0, a literal sets one
field, and they pass by value under the C rules for their size. `va_list` as
a parameter is `Pointer[ffi.VaList]`, which Tondo only passes along. A
pointer macro is `none` when it is `NULL`, or else a function that makes it
with the raw operation `toFunction`, for sentinels such as
`SQLITE_TRANSIENT`. A macro with parameters that calls a function becomes an
`unsafe` function with that function's parameter types.

Without a header, `tondo bind` asks `pkg-config` for a package's headers,
and otherwise reads the library's DWARF with `gimli`: its exported
functions, their types and the constants of its enums, but no macros. If
neither finds declarations it fails asking for the header.

A struct with bit fields keeps its other fields and turns each run of bit
fields into the bytes it spans, with a getter and a setter per bit field
(`flags.mode()`, `flags.set_mode(3)`); the setter panics on a value its
width cannot hold. Bytes are integers to the C conventions, as bit fields
are, so such a struct passes by value as C passes it; only padding in a run
that may share eight bytes with floats keeps it from crossing by value.

## 2026-10-22 — Tests run in workers that restart after a failure

Tests run in worker processes, each running tests one after another as
plain calls; a panic, `testing.failNow`, `testing.skip`, a timeout or a
crash ends the worker, and a new one runs the rest, running again the setup
of the suites they need. A process per test would cost about 0.7 ms on
Linux and would need the values a suite shares to be serialized; `fork` per
test is cheaper but unsafe with C libraries that keep threads, and Windows
has none. Recovering from a panic inside one process would leak what the
test held and could not stop a loop. Tests in one worker share what C keeps
in globals; the runner restores the working directory and the environment
between tests and replaces a worker a test leaves tasks running in. A crash
is its own state, `crashed`, and the run goes on.

Tests see the environment of whoever runs `tondo test` and the project root
as working directory; there is no test plan file and no profile of public
and secret inputs. The testing spec lost what described the earlier
implementation (its VM, bytecode, worker protocol, instruction budgets,
quality evidence), and its formats start again at `/1`.

A suite compiles to a function: its setup, then a call that hands the
runner its members as closures, which capture the setup's `let` bindings
of `Copy` types (`E2005` otherwise), then its `defer`s as its teardown. A
test is checked as such a closure, inferring its errors like a script.
`testing.failNow` is the assertion panic `P0007` where it is called, so its
location is the test's; `testing.skip` is a panic the runner reports as a
skip. Workers report through frames on stderr, and mark stdout at each
frame, so each entry gets the output written while it ran. Panic locations
use a file's path in its package (`src/main_test.to:12:5`).

## 2026-10-22 — A panic cancels every task; where a test's error left

A panic in any task runs that task's `defer`s and then cancels every other
task, which runs its own at its next wait, as cancellation does (11.11,
11.15); the process ends once the main task and the rest have finished, or
after five seconds if one keeps computing without waiting. Before, only the
panicking task cleaned up, so a panic in a task a test started skipped the
test's cleanup and its suite's teardown, and `main`'s `defer`s in any
program.

A test or a setup that ends with an error reports the `?` or `fail` the
error left through, which the runtime learns on that path only; errors stay
plain values without a trace. `tondo test` keeps the test executable and
its tree in the cache, named after everything the build reads, and runs
them without compiling while nothing changes.

## 2026-10-22 — Selection, shards, order, retries and repeats

`tondo test` selects with `--filter`, `--glob` or `--exact`, splits the
selection with `--shard` by the SHA-256 of each identity, and orders it by
identity or, with `--order random`, by the digests of each node's members
under a seed it always prints. Each retry round runs every unit in a new
worker: a failed test, or a suite that failed in its own setup or teardown
with the tests inside it, which absorbs the units within; a pass after a
failure is `flaky-pass` and fails the run unless `--allow-flaky`. `--repeat`
runs the whole plan again in new workers and passes only if every attempt
does. Owners come from CODEOWNERS at the repository's root, the nearest
directory up from the project that holds `.git`.

## 2026-10-22 — JSON and JUnit reports

`tondo test --test-format json` prints the canonical report, `--list` with it
the list, and `--report json=<path>` or `junit=<path>` write them too; a build
that does not compile still gets a JSON report with `compiled: false`. The
schema of the testing spec lost what the native compiler has no use for: a
target's capabilities and the hash of a resource profile. Locations are a
file, a line and a column, which the runtime and people already use, instead
of byte ranges, and `crashed` joins the failures and their counts. Fields of
attachments, snapshots and virtual time are there, empty until those land.

## 2026-10-22 — Attachments and snapshots

`testing.attach` sends its bytes to `tondo test`, which stores them as
content-addressed objects under `target/test-artifacts` (or `--artifacts`)
and writes the manifest after the run. `testing.snapshot` checks against
`snapshots/tondo.snap.json`, which `tondo test` reads strictly before
running and hands each worker; a missing or different value is `P2007` where
it is called, and the report shows the first line that differs.
`--update-snapshots` records them instead and publishes the store only after
a run with nothing failed, skipped or blocked.

## 2026-10-22 — Virtual time in the runtime's scheduler

`testing.withVirtualTime` gives the calling task, and the tasks it starts,
the clock of a domain: `time.now()` reads it, and `sleep`, timeouts and
`select` set their timers on it. The scheduler counts the domain's tasks
that run or can run and its waits on the host; when both are zero it
finishes a `settle`, goes to the next deadline (no further than an
`advance`'s target), or reports `P2003`. A worker runs one test at a time, so
there is one domain at most. The closure gets the `VirtualTime` by value, a
counted handle that only acts while its domain runs; mixing instants of two
domains is not caught; and a wait on a channel or lock shared with tasks
outside the domain counts as one the domain brings. A `P2003` found outside
any task cancels every task, so their cleanups run, as a panic does.

## 2026-10-22 — `std.regex` on the `regex` crate

The runtime links Rust's `regex` crate, built with Cargo as a static library,
and `std.regex` is Tondo over a handful of natives. `regex-syntax` checks the
dialect first, so each error has its kind and offset; embedded flags and
`(?P<name>...)` are refused. Limits are the ones the engine can enforce:
pattern, depth, captures, program size, input, matches, output and
replacement. A step budget, a heap budget and their error kinds are gone, as
the engine runs in O(m·n) and bounds its own memory by the program size. The
first match is `find`, since `match` is a keyword. `findAll` and `replaceAll`
pass over an empty match where the last one ended, as the crate does, so both
see the same matches.

## 2026-10-22 — A file per snapshot, of any value

A single store file put every snapshot on one escaped line, unreadable in a
review, and only took strings. Now each snapshot is
`snapshots/<kind>/<module>/<suites...>/<test>/<name>.snap`, holding exactly
the text compared, and `testing.snapshot` takes any value: a `String` as it
is, anything else as the JSON `json.encode` writes, indented with sorted
keys. `import std.testing` brings `std.json`, so every type gets its
encoding. `ignore` paths turn varying values into `"[ignored]"`, `matches`
also checks their form with a regular expression, and `replace` rewrites
the final text, mostly for strings. The runtime does all of it, on the
`regex` crate. The parameter is `matches`, not `match`, which is a keyword.

## 2026-10-23 — Reflection is code the compiler writes

`std.reflect` reads, writes and calls by name, but the runtime does not walk
type descriptors. Like `json.Encode`, the compiler writes `reflect.Reflect` as
Tondo source for each record, enum and newtype whose parts implement it: a
`match` on the field or method name that reaches it directly, the display,
and the conversion from a value of the same shape. Only the types that reach
reflection are instantiated. A `reflect.Value` is the value in a runtime box,
which knows its type so that `to()` to the same type is exact, with the
dispatch function of its type. `as` is a keyword, so the conversion is
`to()`; `get` and `call` take the value as a copy, since reflected values are
`Copy`. `reflect.convert` between known types is checked after
monomorphization: an impossible shape is `E2110`, a field left `none` is
`W1012`. Method calls now pass their expected type to inference, so
`let x: Int = value.to()?` works, and a `?` on a call that gives a result
infers its success type even when an option is expected. Spilling a
variable for a `mut self` call reloads it with its own type, which a `Bool`
or `Float` receiver needs.

## 2026-10-23 — Type descriptions are code too, and lazy

`reflect.typeInfo[T]()` and `typeOf(value)` describe a type with the same
mechanism as the rest of reflection: the generated `impl Reflect` has a
`reflectDescription` with its kind, fields (and the `///` lines before
each), variants, methods and generic arguments. A `TypeInfo` holds the
identity, name and capabilities of its type and a function that works out
the rest when asked, so a recursive type does not unfold forever. Only the
capabilities the compiler demonstrates are reported: `Copy`, `Equatable` and
`Key`; `Send` and `Share` are not checked yet. A literal path or method of
`reflect.get`, `set`, `call` and `callMut` on a type known at compile time is
checked there (`E2110`); paths and methods computed at run time fail as a
`ReflectError`.

## 2026-10-23 — A prelude in Tondo for the intrinsic types

The everyday methods of `String`, `Char`, `Int`, `Float`, `Option`, `Result`,
`Array`, `Map`, `Set` and every `Iterator` live in `std/prelude.to`, which
every program links with. Only the prelude may declare methods on intrinsic
types (`fn Array[T].map[U](...)`); their owners are keyed by the intrinsic
kind over parameters of its own, so a call on `Array[Int]` finds them. What
Tondo cannot do well goes to the runtime: splitting, Unicode case and
classes, numbers to and from text, and `std.math`. Ordering and sums use the
prelude traits `Ordered` and `Numeric`, which a program can implement; there
is no ordering capability in the language. The prelude's `trait Iterator[T]`
adds default methods to the intrinsic trait: lazy adapters and consumers
that copy the iterator, so they need a `Copy` iterator. Closures passed to
these methods infer their parameters from a partly known `fn(A): B` and wait
for the other arguments and the expected result first. Tuples display as
`(a, b)`, and the intrinsic numeric types have constants (`Int.max`,
`Float.pi`). The standard modules written in Tondo now import the runtime
under distinct aliases, since a package's alias names one module.

## 2026-10-23 — `derive` runs providers as programs

Before the program is checked, each `derive` asks the provider of each trait
it names for an `impl`. The package that declares the trait names the
provider's file in `[derive]` of its `tondo.toml`; the trait is resolved as
the linker resolves imports. The toolchain appends a `main` that serves the
request to the provider's file, builds it for the `meta` profile and caches
the executable per compiler and source. It runs it with an empty environment
in an empty directory, the request in a JSON file and the response in
another, of length-prefixed sections. The `meta` profile rejects the host
modules, `spawn`, `unsafe` and `extern` while compiling (`E2108`); the code
counts a step at each call and each turn of a loop, the runtime's allocator
counts memory when it has a limit, and a run over a budget ends with its own
status (`E2107`). What comes back must parse as exactly the `impl` asked for
(`E2105`); it joins the type's module, and `tondo expand` shows it. The
provider's function is `provide`, since `derive` is a keyword; a `TypeRef` is
the type as the declaration's module writes it, so no imports are needed.

## 2026-10-23 — Generators run before the package is checked

Each `[generators.<name>]` of a package runs as a provider does, with
`generate` served instead of `provide`, before the program is checked. Its
request carries the inputs as bytes with their SHA-256 and the declarations
its roots reach: the roots, and the public records, enums and newtypes their
public fields, variants and wrapped types name, found by following the
imports of each module as written, since nothing is resolved yet. An output
is a module inside the package, and a root a type of one, both written
without the package's name: `outputs = ["palette"]`, `roots =
["models.User"]`. Any problem of a generator, from a panic to
a missing output or one that is not Tondo source, discards everything it
wrote, so a build never sees half a generation. A root that reaches an
output of the same build is `E2109`. A run over budget names the limit and
how `limits` raises it, and a public constant without its type gets the type
to write in the error.

## 2026-10-23 — Ranges are a prelude type

`start..end` outside a `for` header builds the prelude's `Range[T]`, a
record with private fields that implements `Iterator[T]` through a private
`Discrete` trait, so ranges get every adapter without new code generation.
A `for` over an integer range keeps its counting loop; one over characters
uses the record. `x in range` and `c in text` call private prelude functions
that take the element first, keeping the textual order of evaluation.
`String.chars()` walks the text by byte offset with one runtime call per
character, and `Array.iter()` is the lazy counterpart of the eager array
methods. The prelude's intrinsic types show their own names in messages.

## 2026-10-23 — Encoding, digests, randomness and UUIDs in Tondo

`std.encoding`, `std.hash`, `std.random`, `std.uuid` and `std.format` are
written in Tondo over one internal module, `std.internal.codec`, whose
runtime side does the byte work: Base64, hexadecimal and percent-encoding,
SHA and HMAC through the RustCrypto crates, the system's entropy from
`/dev/urandom`, and SplitMix64's mixing. A decoder checks first and decodes
after, so an error names its offset without a status protocol. Tondo's
checked arithmetic cannot step a generator, so the runtime mixes a seed and
a counter, and the generator is a record of both. A `Uuid` holds its
lowercase hexadecimal, which is `Key` and orders as its bytes do; an
`Array[Byte]` is not `Key`. ISO 8601 parsing and `DateTime.sinceEpoch()` are
Tondo in a module that `import std.time` links, like `std.testing` links
`std.json`. Providers and generators can call what is deterministic in
`std.random` and `std.uuid`, but not `Uuid.v4`, `Uuid.v7`, `Random.new` or
`random.entropy`: the checker knows these four and reports the call, and
`v7` reads the clock through the runtime so that `std.uuid` does not import
`std.time`.

## 2026-10-23 — CSV streams through iterators of results

`csv.rows` and `csv.records` are iterators whose elements are
`T ! CsvError`, so a file of any size reads with `for row in csv.rows(file)`
and `row?`, and every adapter applies. A record that spans lines is read a
line at a time until its quotes close; the runtime checks and splits a
record, and the Tondo side tracks lines for errors. `parse` and
`parseRecords` read a whole text through the same iterators over
`io.reader(text)`. The writer quotes only what needs it and ends lines with
`\n`.

## 2026-10-23 — A simple std.log

The old design of sinks, backpressure modes, dynamic log values and
redaction gave way to one `Logger` value: an `io.Writer`, a minimum level,
text or JSON lines, optional timestamps and context fields as
`Map[String, String]`. Each event is one `writeLine`, which `io.Writer`
makes atomic between tasks. A failed write loses the event rather than
making every log call fallible; `flush` reports the stream's error. `with`
is a keyword, so the context method is `withFields`.

## 2026-10-23 — std.http on reqwest, with one handle for runtime values

The API is Tondo (`std/http.to`); the transport is reqwest on a tokio
runtime of the Tondo runtime's own, with rustls on ring, so HTTP/2, TLS,
compression, redirects and proxies come from libraries that are used
everywhere. A task waits for a future the way it waits for host work: the
future runs on tokio and wakes the task when it ends or is dropped, and a
cancelled task drops it. A response's body is a stream of `std.io` over the
body's chunks, so `json()`, `lines()`, `events()` and `copyTo` all stream.
The runtime's objects (client, body, form, response) reach Tondo as one
handle type, `internal.Native`, a reference count and a boxed value, rather
than a handle kind each. Request bodies stay Tondo data until a request
sends them, so a `Body` is reusable except for what a stream reads. TLS
trusts the system's roots, or Mozilla's when the system has none. Status
400 and above is an error that carries the response. A standard module that
imports `std.json` brings its automatic implementations, as `std.http`
does.

## 2026-10-23 — WebSockets in std.http

`WebSocket` runs on tokio-tungstenite with the same TLS roots, offering only
HTTP/1.1 in ALPN since the handshake is an HTTP/1.1 upgrade. Its halves sit
behind separate locks, so one task sends while another receives. Receiving
skips pings and pongs, which the library answers, and turns a close into
the end; a socket is an iterator of messages until then, like CSV rows and
response lines.

## 2026-10-23 — The HTTP server hands requests to Tondo tasks

hyper serves on the client's tokio runtime and sends each request through a
channel to the task that serves the app, which starts a task per request;
the reply goes back through a one-shot channel. No Tondo code runs on a
tokio thread and no callback crosses into Tondo. Route parameters are
`:name` and `*name`, as in Express and Hono, because `{name}` would
interpolate inside a Tondo string. Files, ranges, conditional requests and
uploads to disk run in the runtime; middleware, routing and problems are
Tondo. Networking ignores SIGPIPE once it starts, since `writev` on a closed
connection raises it; programs without networking keep the default. A match
now holds its scrutinee until every arm ran: a later arm that bound the
whole value used to move it out of the frame for the earlier arms too.

## 2026-10-23 — Typed route handlers are written by the checker

`app.get("/users/:id", (id: Int) { ... })` cannot be typed with a function
type, since handlers take any parameters and Tondo has no overloading. The
checker recognises the routing methods of `http.App` with a literal path:
it checks a typed handler on its own, binds it to a hidden local and checks
an adapter it writes as syntax, `(·request) { http.Reply.json(·handler(
·request.intParam("id")?, ·request.json()?)) }`, so every rule and error of
the language applies to it; the expected types of the handler's parameters
pick what `json()` decodes. The route becomes `app.route(...)` with the
operation and schemas of OpenAPI written from the types, as `std.json`
encodes them. Untyped routes with a literal path get their operation too.
The AST derives `Clone` for this.

## 2026-10-23 — No LLM dialect; an API inspector instead

The Tondo LLM Form was measured before building it, on the repository's
171 sources and seven tokenizers (OpenAI o200k and cl100k, Qwen2.5-Coder,
DeepSeek-V3, Llama 3, Mistral Nemo, Gemma 3), with an encoder on the real
lexer whose output kept every file's tokens. Against canonical Tondo without
comments, TLF used 81–88% of the tokens, and Tondo without indentation or
optional spaces, a statement per line, used 84–87%: the saving is the
whitespace, which plain Tondo already allows, and the `;` line was cheaper
only on Gemma. Tabs saved 2%, no indentation alone 8–9%, and a Python-like
dialect without braces only 5%. Words are 53% of the tokens and punctuation
34%, so syntax alone cannot save much more. Declarations without bodies took
26% of the tokens, which is where an agent saves: it reads far more than it
writes. So the dialect and its specification went, `tondo fmt --compact`
prints the compact form, and `tondo api` shows a module's declarations and
documentation, with the compiler's intrinsic modules written in `std/api`.

## 2026-10-23 — The toolchain carries its guide

`tondo guide` is the documentation an agent reads before writing Tondo,
shipped in the binary so that it matches the compiler: an overview with the
rules that differ from other languages and the commands an agent uses, and
a topic per area, short enough to read only what a task needs. Every
example compiles in a test, and every example of a mistake fails with the
code it names, so the guide cannot drift. `--skill` prints the overview as
an agent skill. Writing the mistakes down showed six that the parser and
checker only reported as unexpected tokens or unknown names; each now says
what to write: `if let`, `|` in patterns, `else` on its own line, a keyword
as a name, a C-style `for`, and braces inside a string.

## 2026-10-23 — Panics say what failed and how the program got there

A panic that only named its class (`bounds`) left the reader to rebuild the
values and the path. Now each message carries the values: the index and the
length with the valid range, the operands and the type of an overflow, the
divisor, the shift count, both sides of an asserted comparison, where a
reentrant lock was first taken. And every report has a stack trace. The
compiled code and the runtime keep frame pointers, and the compiler writes a
table from each call's return address to its function and line, so a walk
of the frame chain costs nothing until something fails and needs no debug
information or unwinder. A spawn records the stack it was called from (a
loop spawning from one place shares one record, so spawns cost what they
did), and a task's trace continues through the tasks that started it up to
`main`. A deadlock lists where each task waits, from the frame pointer it
saved when it parked. Stack overflows and invalid memory accesses, which
stay outside the panic model, print the same trace from a signal handler on
an alternate stack before the signal ends the process; a crash inside C is
traced from the program's call into it. Reading a trace allocates nothing,
so the handler cannot deadlock on an allocator the crash interrupted. The
test runner's JSON report fills `stack` from the trace.

## 2026-10-23 — Errors name the fix

A battery of the mistakes that people and models bring from Rust,
TypeScript, Go, Python, Java and C# showed which errors only said
"expected end of line, found `-`" or "unknown name `null`". The parser now
looks at the tokens around a syntax error and names the Tondo form: `->`,
`as`, `<T>`, `[Int]`, `===`, closures with `|x|` or `=>`, `struct`, `class`,
`interface`, `impl P {`, `use`, `async`, `await`, `while`, `loop`, `do`,
`try`, `switch`, `throw`, `let mut` and template strings. The checker does
the same for names, types and methods of other languages (`null`, `True`,
`println`, `HashMap`, `int`, `toInt`, `unwrap`, `forEach`), suggests a
close name for a typo of a binding, function, field, method, variant or
standard module, lists the fields or variants otherwise, and says when a
standard module is not imported. Errors that cascaded are reported once:
`{}` for an empty map, an operator on a value still in an option or a
result. Two silent mistakes are errors now: an `if` without `else` whose
value is used, and `return value` in a function without a result. A root
type named like a type parameter of the prelude, such as `E`, no longer
reports errors inside the prelude.

## 2026-10-23 — One serialization protocol for every format

Before TOML, YAML, MessagePack and CBOR, the traits that JSON owned moved to
`std.serialization`, as its specification foresaw: events that every
format reads and writes (scalars, numbers as written, text, bytes, and the
bounds of lists and maps with their keys), one error type with a path, a
line and a column, a `Source` and a `Sink` that each format gives, and
`Encode` and `Decode` with methods generic over them. Generic methods keep
every call direct after monomorphization; the cost on a JSON round trip of
100,000 records is about 3%. `Source` has three required methods and the
helpers that generated code uses as default methods. The compiler derives
the shared traits, so one type reads and writes every format without
code; a format without null leaves out the members that are `none`.
`json.JsonError` and `json.ErrorKind` remain, as aliases of the shared
ones; `json.Encode` and `json.Decode` are gone, since the language has no
trait aliases, and their users now name `serialization.Encode`.

## 2026-10-23 — TOML on the shared protocol

`std.toml` reads TOML 1.1 with `toml_edit`, the reference parser of the
Rust ecosystem, into the events of `std.serialization`, each with the path,
line and column of its value, and writes documents from events through the
same library, so quoting, escaping and the order of tables are right by
construction. The writer chooses the form: a map in a map is a `[table]`,
a list of maps `[[tables]]`, and a map in another list an inline table.
TOML has no null, so formats now say whether they leave out `none`
members, and a `none` elsewhere is an `Unsupported` error with its path.
Dates and times are a shared event, `DateTime`, which `time.DateTime`
reads and writes; JSON writes it as text. The legacy specification of
`std.toml` described views, validators and canonical encoders that no
program needed; the section now specifies what exists: `decode`,
`encode`, `parse` and a navigable `Value`. A program that uses TOML grows
by about 500 KB, and one that does not by nothing.

## 2026-10-23 — YAML as a safe subset

`std.yaml` reads YAML 1.2 with `saphyr` and keeps the safe subset its
specification asked for: the core schema, so `yes`, `on` and dates stay
text; only core tags; text keys that do not repeat; no merge keys; and no
infinities. Aliases repeat their anchor's value, as a copy of its events
with the paths of where the alias is, and a document stops at a million
values, so a few lines cannot expand into gigabytes. Several documents are
read with `decodeAll` and written with `encodeAll`; `decode` asks for
exactly one, and says how many there were otherwise. Writing uses the
library's emitter, which quotes any text another reader would take for
something else and writes text of several lines as a `|` block; `none` is
written `null` rather than its `~`.

## 2026-10-23 — MessagePack, and bytes for every format

`std.messagepack` decodes with a reader of its own, small enough to give
each failure its byte and path, and encodes with `rmp`, which writes the
shortest form of every integer and length. Because MessagePack writes the
count of a list or map before its items, the shared `Sink` receives counts,
and the writer checks each against the items written: a count that does
not match can only come from an `Encode` written by hand, so it panics
with what happened instead of making every `encode` fallible. Bytes became
a shared type, `serialization.Bytes`, written natively by binary formats
and as base64 by text ones, and the timestamp extension is the shared
`DateTime` event. Keys are text, or integers read as their digits; the
legacy specification's maps of any keys, raw views and canonical encoder
were left out until a program needs them. Generating the decoding of an
enum used a local that a field named `name` shadowed; it is `__variant`
now.

## 2026-10-23 — CBOR in its preferred serialization

`std.cbor` needs no library: the decoder reads every major type with
definite and indefinite lengths, half, single and double floats, the date
tags 0 and 1 as the shared `DateTime`, and the integer tags 2 and 3 when
they fit 64 bits, passing any other tag through to the item it holds; the
encoder writes the preferred serialization of RFC 8949, the shortest head
and the shortest float that keeps the value, with definite lengths. Map
keys keep their order unless `deterministic: true`, which sorts them by
their encodings as section 4.2 asks and rejects a repeated key; the
writer buffers each container's items for that, which costs a copy per
level. The examples of the RFC's appendix A are a test.

## 2026-10-23 — The network without a thread per connection

`std.net` is TCP, TLS, UDP and names over the tokio runtime HTTP already
has. `connect` resolves its host and tries each address, which the legacy
specification forbade; a program that wants to choose calls `resolve`.
A connection's reads and writes go through two `std.io` streams of a new
mode, `Ready`, whose host never blocks: when it would, the task polls a
future in place with a waker that notifies it, so it parks without holding
a worker or a thread, a timeout is a deadline on that future, and a
cancelled task drops it. The stream's operations run again after the
wait, so each changes nothing before its host says it would block;
`read_rest` keeps what it read ahead for that. Over plain TCP the task
reads and writes the socket itself; a write the socket cannot take whole
leaves the rest to a tokio task that sends it in order, and waits only
past 256 KiB pending. TLS moves its bytes in two tokio tasks, and a
listener's handshakes run in their own, so a slow client holds up no other.
Listeners queue as many connections as the system allows, after 2 000
simultaneous clients were found retrying for seconds behind tokio's 1 024.
Reads into a stream's buffer no longer zero 64 KiB each time. `http.Tls`
became `net.Tls`. Measured on loopback against Go: bulk transfer 4.5 GB/s,
one connection's round trip 22 µs (Go 12), 2 000 connections of ten round
trips 0.19 s (Go 0.10); the rest is the hop from tokio's reactor thread to
a worker, which only polling the network from the workers would remove.

## 2026-10-23 — Protobuf waits for gRPC

Serializing Protobuf without gRPC calls would serve the least common use
of it, and the two share one generator: messages and services come from
the same `.proto` files. gRPC also needs what `std.http` does not offer
yet, HTTP/2 trailers, streams both ways and deadlines, since its client
does not expose trailers. Both are a later milestone, designed together;
the standard library's fourth milestone closes with `std.net`.

## 2026-10-23 — `spawn detached`, and a warning for spawns in loops

A task nobody waits for is still waited at the end of its block, which
keeps what structured concurrency gives: a function returns after the
work it started, and cancelling a task cancels what it started. The
surprise was a loop: `for connection in listener { spawn serve(connection) }`
served one connection at a time, silently, which is what a program written
by someone used to Go or JavaScript does first. Releasing a task is now a
form of the language, `spawn detached call()`, instead of
`(spawn call()).detach()`, and a `spawn` whose `Join` nobody uses inside a
loop warns (`W1013`) with both ways out. Making every unused `Join` detach
was the alternative; its mistakes are races and lost work, which tests see
less than a program that is slow or waits.

## 2026-10-23 — Idle workers do not spin

A task woken by the network waits for a worker to wake, which is most of
what separates a round trip over loopback (20 µs) from Go's (12 µs). Idle
workers that spin 20 µs before sleeping, two at most, took a quarter to a
third off that round trip, and cost 3 to 11 % on channels between tasks,
spawning and many connections, whether they spun busily or yielding; that
trade was not taken. The way to Go's latency is the one Go takes, workers
that poll the sockets themselves instead of a tokio thread waking them,
which belongs to a later milestone. The experiment found that a worker
spinning when `main` ended missed the notification and slept for good: any
wait added to the scheduler must look at everything again afterwards.

## 2026-10-23 — Functions compile on every core

Cranelift compiling each function took two thirds of a large build: 0.95 s
of 1.48 s for 43 000 lines in 2 400 functions. The IR of every function is
still built in order, since building it declares what it uses, and then
the functions compile on every core and are defined in the same order as
before, so the executable is the same, byte for byte. That build takes
0.67 s; a small program's, about 0.25 s, is mostly linking. A cache of
compiled functions between builds would save the rest of the compiling,
but with generics its invalidation is subtle, and no build is slow enough
yet to need it.

## 2026-10-23 — Tests in many orders of their tasks

Safe Tondo has no data races, but a test can still depend on which task
runs first. `tondo test --schedules N` runs the selection N times, each in
a fresh worker whose tasks run on one thread: whenever several can run, a
seeded generator picks the next, and each task gives way after every call
that may wait, the points where the order between tasks can change. The
generator also picks among the ready operations of a `select`. A failing
test prints the command that runs its first failing order again, which
repeats it exactly while the test waits only for its own tasks; the clock,
the network and files stay real. With one worker a panic's wait for the
other tasks' cleanups would wait for itself, so that thread runs them.

## 2026-10-23 — A test that leaves tasks running fails

The runner already moved to a fresh worker after a test that left tasks
alive, and said nothing. Such a task, waiting on a channel nobody closes or
running on, is usually a mistake, and it makes the tests after it flaky; the
test now fails, saying for each task where it waits, or that it still
runs, and where it was spawned, from what the runtime already keeps for
deadlocks. Cycles of shared handles, the other way to leak, are left until
`Ref[T]` exists and makes them common.

## 2026-10-23 — Workers poll the network

Sockets without TLS left tokio for an `epoll` or `kqueue` of the workers'
own, through mio. A worker with nothing to run waits on it instead of
sleeping, unless another worker already does, and runs the first task an
event wakes on its own thread, waking no other worker for it, as Go's
scheduler does; the hop from tokio's thread to a worker is gone. What a
socket could not take at once goes when it says it can take more, from
whichever worker sees that, and a stream that lets go of the socket
before then ends its direction after the last byte. Two rules keep the
network attended. A worker with tasks queued looks at it every 61
tasks, so the tasks it wakes do not wait behind a queue that never
empties. A watcher wakes a sleeping worker when no worker has waited on
it for 10 ms, which is Go's `sysmon`: a server whose polling worker ran a
handler 100 ms long answered another client after 95 ms without it, and
after 5 ms with it. The watcher sleeps while a worker waits on the
network. TLS and names stay on tokio. Scheduling a task no longer signals
the condition variable when no worker sleeps. On loopback: one
connection's round trip 12 µs (Go 12), 2 000 connections of ten round
trips 0.13 s (Go 0.11), bulk transfer unchanged.

## 2026-10-23 — `Ref[T]`

A `Ref` is a handle to an object that holds its value after a header of
two counts, strong and weak, so `WeakRef` needs no other object, and the
value's size. `==` compares addresses and hashing takes the address, which
never moves. `reference.value` is a projection through the handle, a place
that compiled code reads where it is, so observing a large or affine value
copies nothing; a read that keeps the value retains it as reading a field
does. Nothing writes through it: an assignment or a method that changes its
receiver says that the value never changes and points to `sync.Mutex`,
and moving an affine value out of it says to use it in place. Encoding a
`Ref` is refused like any handle, since identities are not serialized.

## 2026-10-24 — Cycles of references are collected

Reference counting kept every cycle forever, which the specification
forbids (16.7): a `sync.Mutex` holding a closure that captures it was
three objects lost. A cycle needs something that changes after it is made,
since values, closures and `Ref`s never change, so it passes through
shared state: a `std.sync` cell, a channel's buffer, a task's result. The
compiler works out which types can reach a cycle (a closure, or a cell
whose state can reach a cell of its own type again) and generates, for
those only, a function that reports a value's references, as it already
generates one that releases them. Cells whose state is such a type
register with the collector, closures and reflected values keep the
function for what they hold, and nothing else changes for other types.

A collection starts from the registered cells and counts, for each object
it reaches, the references coming from the others; an object with more is
held from outside, by a task, a variable or the runtime, and lives with
all it reaches. The rest is garbage: its cells give up their state, and
counting frees everything else, so there is no second way to free. A task
still running holds its join and its group without a count, and says so.
The counts must not change while they are read, and a task walking a
graph could move a reference between two reads, so the workers stop at
safe points: between tasks, after calls that may wait and around calls
of C. A task that runs long without one makes the collection give up
after 2 ms and try again. The runtime's own threads never follow the
references of Tondo values; one that frees a registered cell waits.

A collection comes once as many cells were created as objects stayed
alive in the last one, at least 1 024. Six tasks doing nothing but making
cycles outran the collector's thread, and pauses grew to 42 ms; a thread
that registers cells four thresholds ahead now waits for the collector at
its next safe point, and the same run paused at most 0.9 ms. Channels,
spawning and network round trips measured the same as before. Garbage
found is released after the workers resume.

Memory runs out only after a collection: when an allocation fails, or a
`meta` program's budget would be passed, the allocating thread steps out
of Tondo code where it is and waits, up to a second, for the collector's
thread to finish a collection, then tries again. The collection does not
run on the allocating thread, which may hold a lock that freeing garbage
needs; the collector's thread starts with the first registered cell, so
it exists before memory is short, and may itself go past a budget while
it frees. A program making 1.6 GB of cycles runs in a budget of 200 MB,
or under `ulimit -v` of 1.5 GB, where it used to stop.

## 2026-10-24 — `WeakRef[T]` in the prelude

`WeakRef(reference)` and `weak.get(): Ref[T]?` are all there is, with the
prelude's other names in the language specification (26.3), since the
library's may not add any. A weak reference is the `Ref`'s own object with
one more weak count, so making and copying one allocate nothing; `get`
takes a strong count unless the strong ones reached zero. Garbage on a
cycle still has counts until its cells give up their state, so the
collector marks the `Ref`s it found dead while the workers are stopped,
and `get` refuses those: a weak reference never gives back a half-freed
cycle. Weak references have no equality, since what they compare by would
be an object that may be gone; comparing what `get` gives says what is
meant.

## 2026-10-24 — `std.executor`, on tasks the language already has

The legacy contract was written for the old language: `scope`, `await`,
`suspends`, a `Join[T, E]`, pools a program had to close or cancel, a pool
for blocking work. Here a job is an ordinary task, `pool.submit(job)` gives
its `Join`, and the block that owns the `Join` waits for it or cancels it
as any, so a pool needs neither a cancellation of its own nor closing: it
is a handle of two semaphores and a flag, written in Tondo. A job holds
its place and then a worker as permits, which return as the task ends,
also when it is cancelled waiting for a worker. Which waiting job starts
first is not promised. There is no blocking pool, since a call of C that
stays in the system already gets another worker in its place.

An actor is a task reading a channel. Its `Actor` holds the only strong
`Ref` to the sender, its `ActorRef`s weak ones, so `send` cannot find a
channel closed under it, and the end of the owner's block releases the
`Ref` first, which closes the mailbox, and then the `Join`, which waits
for the messages to be handled. Sending uses a private operation of
`std.internal.channel` that reports a closed channel instead of panicking.
`ref` is a keyword, so the handle comes from `reference()`; and since a
library's `self` is a loan, `stop` does not consume the actor: it closes
the mailbox and waits for the outcome the task records, which stopping
again gives as well.

A closure that cannot fail now fits `fn(...): !E`: inferring it with the
error unknown makes it infer its errors, `Never` when there are none, and
`?` on a `T ! Never` only takes the value. The same made the testing
specification's canonical `testing.withVirtualTime(...)?` compile, and a
result that cannot fail may be dropped like `Unit`.

## 2026-10-24 — What the language review found missing

Multiple assignment is lowered to what exists: the places are resolved
left to right with their indices held in temporaries, the value is
destructured into hidden bindings, and each place is written in turn, so
swapping values that are not `Copy` works because writing a moved
binding fills it again. Places that overlap before the program runs (the
same binding, field or constant index, or one inside the other) are
`E1405`; swapping affine elements of a collection is not offered. Single
variadic arguments may come before a spread, which concatenates them
with it when the elements are `Copy`. A script, a test or a closure that
infers its errors now infers the union of all it propagates, where the
first one used to fix it. `defer` takes calls through function values,
as `spawn` already did.

`Send` is checked where a value crosses between tasks: what `spawn`
takes and gives back, a channel's values, a closure that becomes a
`fn(...)` value, which any task may call, and a type argument for a
parameter declared `T: Send`. Only a `Pointer` and what holds one fail it
in 0.1, and a type parameter without the bound is taken as `Send`, so
generic code instantiated with a pointer is not caught.

## 2026-10-24 — The `core` warnings

A warning is for the code its reader can change: those about the standard
library, the dependencies, and what derives and generators wrote are
dropped. Imports, naming and confusable names are checked as the linker
walks each file, before declarations are renamed; unused bindings and
parameters as the checker resolves names, where a read is a use of the
binding by name and an assignment is not, except through a `mut` or `var`
loan. A name that starts with `_` is not reported, so a binding or a
parameter that holds a permit or a lock until its scope ends has a way to
say so; `_` alone binds nothing, as 5.4 says, so a function may take
several.

Names follow 5.4 with acronyms as words: a run of capitals before a word
ends one word at its last capital (`HTTPClient` is `Http` and `Client`,
`UInt8` is `U` and `Int8`), and the warning gives the conventional name.
What mirrors C keeps its names: `extern` functions and types, and a module
`tondo bind` wrote, known by the line it starts with. Two names are
confusable when their UTS #39 skeletons match and one is not ASCII, as
`unicode-security` computes them for Unicode 16.0.0; between two ASCII
names the skeleton would confuse `modern` with `modem`.

Writing the cases found that a discarded index, `_ = values[9]`, did not
check its bounds, because a place that is never read was never loaded;
lowering now reads it.

`E1504` replaces `E1002` for a method on a type the module does not
declare, which a qualified owner, `fn models.User.greet`, now reports
instead of a syntax error, and `E1505` for fields, variants and methods
that clash. A `#!` below the first line, or one starting a file that is
not a root script, is `E1805`.

## 2026-10-24 — Interrupting `tondo test`

Each worker runs in a process group of its own, so a Ctrl-C reaches only
`tondo test`. It stops dispatching tests and sends each worker `SIGTERM`,
which a test build answers by cancelling all its tasks, the one running
the test too: their `defer`s run at their next wait, and the worker
starts no other test. A worker that ignores it, in a loop that never
waits, ends when its own grace after a cancellation does (5 seconds,
killing the processes it started) or when the runner's does (10 seconds,
killing its group), or at a second Ctrl-C. Exit `4` says the workers
ended by themselves, `3` that one was killed. What the interruption cut
has no outcome, the human output shows what ran and says it was
interrupted, and no report, JUnit file or snapshot is written.

## 2026-10-24 — `tondo doc-test`

The fixtures of Appendix C are written once, in the manifest the
appendix names, `conformance/0.1/fixtures/tondo-fixture-manifest.txt`,
whose hash every record carries. The runner turns its lines into Tondo:
an opaque type is a record without fields, or one holding a `Join` when
it is `Send` but neither `Copy` nor `Share`, so moving it is checked as
the language checks any affine value; an operation gets a body that is
never run. A fixture's declarations go at the end of the example, as the
appendix says, and its modules replace the standard ones of the same
name, except `std.console` and `std.process`: what the manifest declares
of them is part of what they have, and a pipeline only runs through the
real one. An operation a fixture declares on a type of the language is
used only where the toolchain has none, and a type the prelude has, such
as `Deque`, is the prelude's. An example without `main` or statements is
checked as a library.

The examples showed that the language had no closures called once: one
that gives a capture away is now `CallOnce`, a call consumes it, and it
cannot become a `fn(...)` value or satisfy `Call` or `CallMut`. Only the
check changes: reading a capture still takes its own reference, which the
closure gives up when it goes.

They also showed two places where the specifications disagreed with
themselves: the `tondo` category took no imports, which fifteen examples
of the standard library and one of the language write before their
statements, so a syntax sequence may now begin with them; and
`spec.application` took an `Array[env.Value]` that `env.args()` does not
give, so it takes an `Array[String]`. The examples of the four
specifications pass, and a test checks them.

## 2026-10-24 — Tondo links ELF executables itself (H12, first part)

The linker reads the program's object, takes from the runtime's archive
and from native archives only the members that define what is missing,
and binds the rest to shared libraries found where the system keeps them
to run programs, by the names they run with (`libc.so.6`, `libm.so.6`,
`libgcc_s.so.1`; musl's one library), so no development package is
needed. What shared libraries define is bound when the executable
starts, all at once (`BIND_NOW`), through a GOT and PLT entries that jump
through it, with the versions each library defines. Sections that
nothing reaches from the entry and the constructors are dropped, the
runtime's thread-local accesses are relaxed to the executable's model,
and `.eh_frame` keeps the descriptions of the functions that stay, which
`.eh_frame_hdr` indexes. The runtime brings the entry, `tondo_rt_start`,
which calls the C library's `__libc_start_main` with a function that runs
the constructors, as the C toolchain's start files do. `tondo build
--strip` leaves out the symbol table; Tondo's traces use their own table.

Executables are position independent, as a C toolchain makes them, so
the system loads them anywhere and attacks cannot rely on where their
code is. The code costs nothing more, since it is already position
independent; the words that hold the executable's own addresses move
with it when it starts, listed as `DT_RELR` bitmaps where glibc reads
them (it defines `GLIBC_ABI_DT_RELR`, which the executable then asks
for) and as relocations otherwise.

## 2026-10-24 — Static executables (H12, second part)

`--static` on `build`, `run` and `test` links a static executable on
Linux: the runtime built for musl, musl's C library and its unwinder,
which Rust ships for its musl targets and the compiler carries. Tondo's
linker links them with no loader and no dynamic section; the executable
starts in musl's `rcrt1.o`, which relocates it and calls the runtime's
`main`, as a C toolchain's start files would. Shared libraries cannot
come in: a static executable has no loader, musl's `dlopen` needs one,
and a library built for glibc would bring a second C library into the
process. Archives can; one built for
glibc links when it uses nothing only glibc has, and the error says when
it does. The compiler carries both runtimes compressed, at about a
quarter of their size, and writes each to the cache the first time it
links. Where there is no C compiler for musl, the runtime's C code for
musl is compiled by the system's, without `_FORTIFY_SOURCE`, whose checked
functions musl lacks.

## 2026-10-24 — Tondo links Mach-O executables itself (H12, third part)

On macOS the linker cuts each section of the objects into atoms at their
symbols and keeps the atoms the entry and the constructors reach, as
`-dead_strip` does. What `libSystem`, the frameworks and other dylibs
define is bound when the program starts, by name, in whichever library
the program loads that has it (flat lookup), so linking needs no SDK and
no `.tbd` stubs: a dylib is named by the path it has in the system, where
it no longer exists as a file. The executable is position independent,
as macOS requires; thread-local variables are descriptors that `dyld`
initializes; `__unwind_info`, rebuilt from the objects' compact unwind
entries and pointing into `__eh_frame` for the functions only DWARF
describes, lets C++ exceptions and backtraces cross the program; and
an ad-hoc signature, which arm64 requires before it runs any code, covers
the file.

## 2026-10-24 — Executables without a C toolchain (H12)

`tondo` links every executable with its own linker, on Linux and macOS.
`TONDO_CC` names a C compiler to link with instead, should a system need
it. CI runs the cases and projects in containers with
no C compiler, Debian and Alpine, dynamic and static, and the cases once
more with `TONDO_CC=cc`. `tondo bind` still reads headers with a C
compiler's preprocessor.

## 2026-10-24 — Counters of running programs (H13, first part)

A Tondo program started with `TONDO_DIAGNOSTICS=1` serves the tools that
watch it on a Unix socket, `$TMPDIR/tondo-<uid>/<pid>`, in a directory only
its user can open, as .NET's diagnostics do; without it, the program opens
no socket and counts nothing for them, so that one nobody watches pays for
nothing but a load where each count would be. Attaching from outside, with `ptrace` or
`task_for_pid`, needs permissions that macOS does not give and Linux
restricts. A runtime thread of its own answers every tool with `poll`, so
a program whose workers are busy or stuck still answers, and neither its
sockets nor its memory count in what the program shows. `tondo ps` lists
the programs and removes the sockets of those that died without removing
theirs; `tondo counters` samples one every second, named by its process or
its file, as a table or as JSON lines with the rate of each total. The
runtime counts memory, objects, the collector's pauses, tasks, workers and
their calls of C, timers, the bytes of `std.net` and HTTP requests;
sockets, files and pipes come from the process's descriptors, so those of
C libraries count too. `std.diagnostics` gives a program counters and
gauges by name.

An atomic operation for each allocation would cost dearly on many cores.
The count of live objects that the leak check reads already was one
atomic for the whole process, and a program that allocates on four tasks
ran 40% faster without it. Each thread now counts in a block that only it
writes, with no atomic operation, and the tools read the sum of every
block; a runtime thread's block goes to the next one that starts. While
nothing watches the program, an allocation, a free and each object read
one word of flags inline, the same word that says whether a `meta`
program keeps to a budget of memory, and nothing else runs: a call to
code that would return at once, or two flags read apart, had cost that
program 4%. Watched, it runs about a fifth slower, every allocation
counted and some sampled. The leak check still counts objects in one
atomic, so that the count at exit is exact.

Measuring it showed that the program's time on four cores changed by
nearly half with where its memory lay: a single allocation of a few
bytes before `main`, as reading an environment variable makes, moved it
from 150 ms to 280 ms, while on one core it did not change. glibc makes
a thread's cache of freed blocks, whose counts change at every
allocation, at the thread's first free, right after the block freed.
That block, allocated again, became the closure of the first task, which
the thread that ran the task freed and then reused for a text builder at
every interpolation; when the counts and the block shared a cache line,
the main thread and that one took it from each other at every
allocation. The runtime now frees a block and allocates it again before
anything else, and keeps it, so the main thread's counts follow a block
nobody uses. The time also moved by a tenth with where the runtime's
functions lay, whenever one of them changed; each now starts a cache
line, which costs an executable 8 KB, and that program takes 147 ms
whatever its layout.

## 2026-10-24 — Traces of running programs (H13, second part)

While a tool traces it, the runtime records each run of a task on its
thread, where the task waits when it stops, each spawn with the function
the task runs, the collector's pauses, calls of C of a millisecond or more
and panics, every thread into a buffer of its own; while nothing traces,
each of those costs a load. Every millisecond of processor time, `SIGPROF`
samples the stack of the thread that runs into a ring made in advance,
which a signal handler can write without allocating or locking; samples
come as often as the kernel's clock lets them, every 4 ms at 250 Hz. The
tool takes what was recorded ten times a second, over the same socket, and
writes Trace Event Format, which Perfetto opens, and folded stacks, which
flame graph tools read: formats that people and models already know. The
runtime names the program's frames from its table of functions; other
frames carry the file and offset `dladdr` gives, and the tool names them
from the file's symbols. The body of `spawn f()` is now named `spawn f` and
kept in that table, so a trace names a task by its function; panic traces
still skip it. A program that `tondo trace -- program` starts waits for the
tool before `main`, up to five seconds, so the trace covers all of it.

## 2026-10-24 — Dumps of running programs (H13, third part)

`tondo dump` asks a program for its tasks, counters and live memory, and
`TONDO_DUMP=<dir>` makes a program that panics, faults or deadlocks leave
the same dump there; `tondo dump read` shows one. It is JSON in a format of
Tondo's own: an operating system's core dump on macOS needs entitlements,
and reading one would need to know the runtime's structures anyway. To show
where a running task is, the workers stop at their next safe point, each
noting its frame, as for a collection; a worker that reaches none, as a
loop that only computes, is asked with `SIGPROF` and answers from the
frame the signal interrupted. A task inside C shows the frame that called
it.

Live memory comes from samples that the runtime takes while it is
watched, as Go does: an allocation for every 512 KiB on average, at random
intervals so that no size is favoured, with the stack that made it, kept
in a table of four entries by address while it lives. An allocation costs
a thread-local countdown, and a free a byte read of its bucket. A sample of `n`
bytes stands for `n / (1 − e^(−n / 512 KiB))` of them, and its site is the
program's code past the standard library. Its type is the one the statement
made, or the collection it changed: the compiler gives each statement its
own source location and writes the type beside each call in the table of
traces, so naming it costs nothing when the program runs. Objects carry no
type, so who holds one waits for debug builds, where a header can say.

## 2026-10-24 — A debugger for debug builds (H13, fourth part)

`--debug` compiles a debug build: unoptimized, with a probe where each line
starts, and a table the runtime reads that names each function's bindings
and their types, with a function that shows each type: texts in quotes,
records with their fields, enums with their variant, collections with
their elements. A build without `--debug` has none of it. A probe passes
the addresses of the function's bindings and a bit for each that holds a
value: the code sets it when a binding is assigned and clears it when its
value moves or is released, so the debugger never reads memory that is
gone. A binding in a variable is copied to the stack when a probe runs.
While no debugger is attached a probe reads one flag; a stopped task waits
outside Tondo code, so collections and dumps go on, and the others stop at
their next probe. The debugger lives in the program and speaks over the
diagnostics socket, so it needs no `ptrace` or Mach ports, knows the tasks
for what they are, and shows values as Tondo writes them; gdb and lldb see
neither. `tondo debug` reads commands from a person or from its input,
with `--json` answers for tools and models; `tondo dap` turns the Debug
Adapter Protocol of an editor into the same commands. A program that
`tondo debug` starts waits before `main` from the moment the debugger
attaches, so that its first `continue`, however soon it comes, is the one
that lets it go.

## 2026-10-24 — Every object in the dump of a debug build (H13)

A release build's dump samples the live memory and names each sample's
type at no cost when the program runs; who holds an object needs every
object, which only a debug build pays for. While it is watched, its
allocator notes each allocation that compiled code made, with the call
that made it, whose type the table of traces names. A dump then reads
every object word by word for pointers to the others, and the tasks'
stacks for the objects they hold, so `tondo dump read <file> object
<address>` gives the shortest chain that keeps one alive, from a task's
stack. Pointers are found by their value, so a number that equals an
address counts as one; for a debugging tool that errs on the side of
keeping, which is the side that answers "why is this still here".

## 2026-10-24 — Libraries for C (H14, first part)

A Tondo package can be a library that a C program links. What C calls is
an `extern fn` with a body: the keyword already means C's calling
convention and a symbol, and `pub` already means published, so a library
publishes the `pub` ones of its package and nothing else needs a name.
Their types are those that cross to C as callbacks already do, and the
adapter that runs a callback runs them, with the symbol exported. A
library is asked for with `--library` rather than inferred from a missing
`main`, which scripts are to have a use for.

`tondo build --library` writes `lib<name>.a`, the program's object with the
whole runtime, and `<name>.h`, written from the signatures: C's names for
`std.ffi`'s aliases, `stdint.h`'s for Tondo's numbers, the `extern type`s
the functions reach, after those they hold, and the `///` documentation.
The archive is the runtime's as Rust builds it, 65 MB; linked with
`--gc-sections`, as the line the build prints does, a program that calls
it takes 1.2 MB without debug information, as a Tondo executable does.
A runtime in an archive brings no `main` of its own: the compiler writes
an executable's `main`, and the start code that the toolchain's own
linker uses refers to `main` and to `__init_array_start` weakly, through
the GOT, so a shared library made with the C toolchain links it too.

The runtime starts at the first call. A call from a thread of C used to
become a task that a worker ran while the thread waited; now the task
starts on the calling thread and moves to a worker only if it waits: a
call that returns a sum takes about half a microsecond, where the trip to
a worker and back took 7. The output a library writes is delivered when each call
returns, so it comes before what its caller prints next. Faults outside
the library's tasks and code go to the handlers the process had before,
since hosts such as Java's take faults on purpose; and a thread of C keeps
the signal stack it has. Two Tondo archives in one program would bring two
runtimes with the same symbols, so a program links one.

## 2026-10-24 — Shared libraries for C (H14, second part)

The toolchain's own linker makes the shared library, as it makes
executables: the roots are what the library publishes, which its dynamic
table holds and nothing else of the runtime, so a process loads several
Tondo libraries, each with its own runtime and none in another's way. The
library reaches its thread-local variables through the loader, with
`__tls_get_addr` on x86-64 and TLS descriptors on arm64, rather than in
the block a program reserves when it starts, so `dlopen` loads it at any
time. It is never unloaded: the runtime's threads run its code. On macOS
it is a `MH_DYLIB` whose install name is `@rpath/lib<name>.dylib`, with an
export trie of what it publishes. The shared library of the example
program takes 1.2 MB, as its executable does.

## 2026-10-24 — A lighter runtime

Asked how heavy a library's runtime is, the measure was: 1.1 ms for the
first call, 31 threads and 6 MB more than the C program had, all for a
call that adds two numbers. Three things made it so, and none was needed.
The first call started a worker for each core; now a call from C starts
them only if it waits, as a spawn already did, and a call that returns at
once leaves the process with no thread of the runtime's. The samples of
live memory and the ring of a trace's samples were 8 MB of static tables
in every program; they are mapped when a program is first watched. And
where the system backs memory with huge pages whenever it can, the top of
a task's stack took a 2 MB page for the few kilobytes it uses; stacks now
ask for small pages. The library's first call takes 46 µs, it adds 0.8 MB
to its host, and nothing runs while it waits for calls; a program that
prints a line takes 3 MB instead of 7, and one with a thousand tasks that
wait, 8 MB instead of 63 to 84. The four-task program that allocates went
from 146 ms to 157; builds that differ from this one only in code that
does not run take from 157 to 163, since Rust's standard library comes
compiled and where its functions fall moves with the code before them.

## 2026-10-24 — Under qemu

Tondo for arm64 under qemu's user emulation, as `docker run --platform`
runs it on an x86-64 machine, failed seven of the cases that pass on arm64
hardware; they had been taken for the emulator's. Each had its cause.
qemu 11.1.1 gets `smulh` wrong when one factor is a 1 it knows when it
translates the code and the result goes to the register of a factor: five
cases multiplied by `sizeOf[T]()` of a byte, which panicked as an
overflow. The compiler no longer multiplies by a constant 1, which is the
other factor and cannot overflow. qemu also has a thread of its own,
which the runtime took for a thread of C that could still wake a program
whose tasks all wait, so a deadlock was never reported; the threads an
executable has before its program starts are now not counted. And a
debug build handed its table to the runtime after the diagnostics socket
existed, so a debugger that asked at once, as one under emulation does,
was told the program had none; the table now comes before anything else.
The cases now pass under emulation for arm64, dynamic and static, and for
x86-64, and so do the other tests, but two that need `git` and `kill`,
which the container did not have.

## 2026-10-24 — Targets

`--target x86_64-linux` and `--target aarch64-linux` build a static
executable for Linux on either architecture from any machine, as `--static`
builds one for this machine: Cranelift has both backends, and the linker is
the toolchain's own, so the target's static runtime and musl are all a
build needs. Go builds its runtime from source for any target; Tondo's is
Rust and C, which a user does not have the compilers of, so another
target's runtime comes built, as Rust's `rustup target add` brings it.
Carried in `tondo`, each would add 17 MB to its 50; it carries its own
machine's, and another target's comes in a pack, the runtime, musl and
the identity of the runtime's source, which `tondo target pack` writes on
a machine of that target. `TONDO_TARGETS` finds packs; downloads of them,
fixed by hash as dependencies are, come with releases. A program for
another architecture runs under qemu's user emulation, `qemu-<arch>` from
`PATH` or what `binfmt_misc` has, so `tondo test --target` tests it here.
The CI builds `tondo` across, tests it under emulation, and builds for
the other architecture with the pack of the emulated `tondo`.

## 2026-10-24 — Agents

`tondo mcp` serves the toolchain to agents over the Model Context
Protocol, from the compiler's own binary, as `tondo lsp` serves editors:
it needs nothing else installed and always matches the compiler. It
speaks both eras of the protocol, since clients still open with
`initialize` while 2026-07-28 drops the handshake for a version on each
request. The commands' tools run `tondo` itself, so they behave and fail
as the commands do, a crash or a hang does not take the server down, and
a timeout ends the whole process group; `check`, `fix` and navigation
analyze in the server, since they need the spans. Places count lines and
columns from 1, columns in characters, as diagnostics print them, which
the language server now does on request, publishing nothing. A debugging
session is a handle the server gives, as the protocol advises for state,
and no call waits on a running program longer than `wait`: `pause`
interrupts the debugger as Ctrl-C does. The results carry their data
without an `outputSchema`, since the JSON of the commands is what the
specifications define. `std.mcp`, for programs to serve their own tools,
and reloading a running program for agents are later milestones.

## 2026-10-25 — Setting agents up

`tondo setup` gives the coding agents of a machine the skill and the
MCP server, and checks that `tondo` compiles, runs a program and serves:
a `tondo` downloaded alone has no repository and no script to do it, and
the repository's script now only builds or downloads it and calls the
command. It registers the server with the agents' own commands, `claude`
and `codex`, since their configuration files are theirs to write, and
with the path of the `tondo` that runs it, since an agent started from a
desktop does not see the shell's `PATH`. It finds an agent by its command
or by its directory in the home, writes the skill where each reads
skills, and replaces what it registered before, so that running it again
updates everything.

## 2026-10-25 — Packages without `main`

A package of declarations only, as one others depend on, gave E1806 to
`tondo check`, to editors and, against testing 6.3, to `tondo test`,
whose check of the tested sources on their own asked for `main`. Only
building or running a program needs an entry point now: `check`, the
language server, `tondo mcp` and the tests check such a package as it
is, and `run` and `build` still say there is nothing to run, and that
`fn main() { ... }` or statements at the top of the file give it
something.

## 2026-10-25 — Live builds

`--live` builds a debug build whose code changes while it runs, under the
debugger or not, so that an agent or a person tries a change on the state
a program already has. Every call of a program's function loads its
address from a slot, one per function and signature, and the image keeps
tables of the runtime's functions, which it links all of, of the functions
of C it calls and of each type's identity. A reload compiles the whole
program again from the directory it was built in: Cranelift makes that
fast, and every later call then runs one consistent version. The
toolchain's own linker lays the object out as an image the runtime maps
anywhere, whose references to the running image are entries the runtime
fills from those tables, so no symbol of the executable is read and moving
it does not matter. The old code is never freed: frames still in it finish
there. A type whose fields or variants change is refused, since values of
the old shape are alive; `--watch` runs the program again instead. A
debug build's standard output now comes at every line, as to a terminal,
for the tool that reads it. Restarting a stopped frame with the new code
comes later.

## 2026-10-25 — What a reload changes

A closure made before a reload ran its old body, so a server whose handler
is a lambda never saw a change: closures and `defer`s now call their
bodies through slots too, and a closure's identity is its place among its
function's closures, how many those are, its signature and the shapes it
captures. A type changes shape in place when no running frame can hold a
value of the old one: Tondo has no globals, so then no such value exists
and nothing needs copying. The runtime stops the program, looks at every
task's stack against the types each function's frames may hold, and puts
the patch in before it goes on; otherwise it names the frame. Identities
carry the shapes their signatures reach, so that old code that remains,
as a closure that captured old values, keeps its version. Copying values
into a new shape while code that reads the old one runs needs on-stack
replacement: a milestone of its own, after the one that moves frames to the
new code of a function without type changes. `restart` returns from the
stopped frame as `return` would and calls its function again with the
same arguments: parameters are borrowed, so they are still there, and the
probe of the line answers whether to do it, so a live build checks for it
only where a debugger can stop.

## 2026-10-25 — Frames move to the new code

A reload left every running frame on the old code, so the loop of a
server's `main`, which never returns, never saw a change. A frame of a
changed function now moves to the new code at the next statement it
starts, on its own: each statement start of a live build checks, at its
probe, whether the patch moves frames from there, and if so writes the
values live there to a buffer the runtime lays out as the new code expects,
calls the new function with its arguments and that state, which enters at
the same statement, and returns what it returns. The old frame stays
below, waiting, and stacks leave it out. Statements match by text, not by
line numbers: the lines of the function that stay the same are matched by
their longest common sequence, a changed line takes the place of the old,
and the statement there must be of the same kind. Values match by binding
name, by order among the bindings of one name live at once, and those the
compiler makes by the statement that makes them, and each must keep its
type. When one does not match, a binding is lent, a value would not be
released or the `defer`s differ, the frame stays on the old code until it
reaches another statement, and reload says where and why. A patch now sets
only the slots of the functions that change, so that a function a later
reload changes still has the points of the code its frames run. A move
enters the code its state was made for, which the runtime records with the
move, so a patch that comes meanwhile does not mix two versions. Values of
a type whose shape changes still need starting again: copying them into
the new shape is the next milestone.

## 2026-10-25 — Less under emulation

CI took about 50 minutes, and 37 of them were the cases run three times
under qemu's emulation of x86-64 on arm64: as they are, linked statically
and as debug builds, about 12 minutes each where natively they take less
than one. The static and debug runs repeat what the test jobs of both
architectures already run on their own hardware, and the musl jobs
statically too. Under emulation the tests now run once, which still checks
that the `tondo` built across, for glibc 2.28, works where it was not
built.

## 2026-10-25 — Values of a changed type convert

A reload that changed what a type is made of was refused while a running
frame could hold a value of it, which a server's state always is. Values
have no identity (4.1 of the language), so a value of the new shape made
from the old one is that value: a frame converts the values it holds as
it moves to the new code, and until then it runs the old code with the
old values, so the two never meet. The running build's manifest
describes each type its points hold, by the keys of the types it is made
of; the patch rebuilds the old types from it as types of their own, of
the same names, so their layouts are the old ones, and writes for each
pair a function that converts a value or gives `none`, as the checker
would write it, so that layouts, references and the release of what a
failed conversion made come from the compiler as they are. Fields go by
name, variants by name; a new field, or one of another type, starts with
its type's empty value, and one without, as an enum, keeps the frames on
the old code with a hint to make it optional. A value of a variant that
is gone is only known when the frame moves: the conversion fails, every
value made so far is released, and the frame stays, which the program's
error stream says. Values with identity, as a `Ref` or a channel, would
need every frame that may reach them to move at once; a reload is
refused while a frame may keep a value of a changed type in one.
Frames of a function whose parameters or result change still finish on
the old code, and numbers do not convert to other numbers.

## 2026-10-25 — Frames move with their parameters converted

A function whose signature reaches a type of another shape is another
identity, so that callers and callees always agree, and its frames used
to finish on the old code: a server's `serve(config)` loop never saw a
change to `Config`. Such a frame now moves to the identity of the same
name and signature, with its parameters converted. Parameters are the
caller's, so the old frame hands them to the runtime, a word each, the
runtime gives the call of the new code converted copies, and releases
them once the call returns, to the old frame that then returns to its
caller. The result must be the same, as the old caller takes it, and a
parameter lent with `mut` or `var` must not reach a changed type, as it
points at the caller's value of the old shape. A reload reports only the
changes of shape it makes, while its conversions still reach frames that
earlier reloads left on older versions.

## 2026-10-25 — Numbers convert, closures follow

A field that became another numeric type started empty; it now converts
as `T(x)` does (8.6 of the language): always when the new type holds
every value of the old, and otherwise when the value fits, a frame whose
value does not staying on the old code until a statement where it does,
rather than losing it. A closure's identity carries its place among its
function's closures, so that editing its body keeps it; adding another
closure to its function renumbered it, and its frames finished on the old
code, as a server's handler loop did when `main` gained a lambda. The old
closure is now matched to the one of the same function and signature
whose text is most like its own, by the pairs of characters they share,
which a one-line lambda keeps through an edit as lines would not; its
frames move with their captures converted, unless the closure changes or
gives its captures away, as it then works on the closure's own; and when
only its number changed, the old identity's slot leads to the new code,
so values of it made before run the new body.

## 2026-10-25 — Renames keep values

A field renamed was a field gone and a field new, its values lost. The
language has no annotation to say so, and asking for one would go
against doing things by themselves; the edit itself says it well
enough. When a record loses as many fields as it gains, and each lost
one's values convert to the gained one in the same order, they are the
same fields renamed; variants pair the same way. A type the program no
longer has is the one it has newly, of the same kind and most of the same
field or variant names, if no other is as close; functions whose
signatures named the old type are then the same functions. The report
says each rename, so a pairing that was not meant shows.

## 2026-10-25 — What goes back to the old code converts back

Three kinds of frames finished on the old code because something of the
new shape would go back to the old: a result its old caller takes, a
value lent with `mut` or `var`, which is the caller's, and the captures a
closure changes, which are the closure's. Each now converts back, with the
same conversions the other way. The new code writes its result in the
new shape where the runtime says, which converts it into the caller's
place; a lent value, or captures, are copied in converted and, as the
loan is the call's alone while it lasts, the copy goes back converted
over the caller's when the call returns. Only a way back that loses
nothing is taken: every old field comes from a new one, every new variant
is an old one and every number fits; otherwise the frame finishes on the
old code, as before, and the report says why.


## 2026-10-25 — Values with identity convert in place

What a `Ref`, a channel or a `std.sync` value holds is shared between
tasks, so a copy of the new shape cannot replace it: it converts once, in
place, and no old code may touch it afterwards. A reload that needs it
stops every task at the start of a statement, as a debugger pauses them.
A task that waits does not get there by itself, and a server spends its
life waiting: when its wait has done nothing yet — a receive, a send to a
full channel, a lock, a sleep, a line of input — the wait ends and the
statement starts again with the new code, repeating what it did before
waiting, as evaluating its arguments. The alternative, waiting until each
task gets to a statement alone, changes nothing visible but would almost
never happen. Waits that did something, as `updateWhen`, which let go of
the lock, or `Barrier.wait`, which counted the task, do not start again.
The compiler records, for each call, what the rest of its statement may
use through such values; a frame whose rest uses them, other than passing
them to the program's functions, which run the new code, and whose task
cannot start the statement again, refuses the reload, and so does a lock
taken. A live build keeps a `Ref`'s value apart, one indirection more,
where one of another size fits, and remembers each `Ref`'s release glue,
so that old code releases a converted value with the new glue.

For a line of input to be such a wait, the console's input became a
stream like a socket's: a host thread reads it when a task asks, and the
task waits without holding a thread. A cancelled task waiting for a line
now stops at once, as 11.15 of the language says, and what arrives stays
for the next read.

## 2026-10-25 — Old code stays

A live build never frees a patch's code, and a frame that moves to new
code leaves the old one below it, returning what the new one returns.
Measured over a hundred reloads of a small program, memory grows by
about 13 KB each; a move costs one frame of a task's 8 MiB stack. Freeing
a patch would need to prove that nothing points into it: string literals
of its code are immortal and may be in any value, closures keep the code
of the patch that made them, and the frames that moved are still below.
Not leaving the old frame below would need tail calls, and so Cranelift's
`tail` calling convention for every function of a live build, with
adapters for all the runtime calls. Neither is worth its cost while only
a reload during very deep recursion would notice.

## 2026-10-26 — The compiler and the linker on every core

A build must give the same bytes whatever the machine (8 of the
toolchain), so work spreads over the cores only where items are
independent, and their results come back in the items' order: the
linker reads the archive members a round needs, the relocations of the
sections it reaches and relocates the sections on every core, and takes
their symbols in, in order; Cranelift compiles functions, and the parser
reads files and standard modules, the same way. The threads are made
once and wait between calls, since many calls have less work than making
threads costs; a call from inside another runs on its own thread. A test
builds a program twice and compares the bytes.

Measuring first showed that the largest costs were not a lack of cores:
reading the runtime's 65 MB archive at every link (now mapped), a lexer
that copied every token and every identifier, a parser that copied
tokens to move past them and climbed eleven levels of precedence for
each operand, and two checks for names that look alike that compared
each name with all others. A program using `std.http` and three
encodings builds in about 70 ms instead of 120; one of 24,000 lines in
about 430 instead of 770.

Building each function's IR on another core meets declarations made as
the IR asks for them, a string, glue, a function of the runtime, a debug
build's sites, which number the object's symbols in the order they come.
A worker builds a function with names of its own for those, recording
what it asked in order; then, function by function in their order, the
asks are declared as one core would have declared them, and the
function's references renamed. The object is the same, byte for byte,
which every case of the tests confirms in each kind of build. A function
that calls C is built again in its turn, since whether it calls a symbol
directly depends on the calls before it.

## 2026-10-26 — The VS Code extension

The extension lives in this repository, in `editors/vscode`, so that it
changes with what it drives: everything it does is `tondo`'s, through
`tondo lsp`, `tondo dap`, `tondo test --test-format json` and `tondo mcp`,
and it only connects them to VS Code. It is TypeScript with one runtime
dependency, the official language client, bundled with esbuild into one
file. What does not need VS Code, finding `tondo` and reading the tests'
list and report, is tested with Node's own runner in CI, which also makes
the `.vsix`; running VS Code itself in CI would test VS Code more than the
extension. It is not published to the Marketplace or Open VSX yet: that
needs a publisher account, which is the maintainer's to create.

Debugging one test needed `tondo dap` to build a project's tests and run
one of them: `launch` takes the test's id, builds the tests with debug
information as `tondo test --debug` would, gives the executable a plan of
that test alone, and turns the runner's frames into a line that says how
the test ended.

## 2026-10-26 — The command line for tools

Every command takes `--json` and writes one object, `{"format":
"tondo-<command>-0.1/1", "ok": ..., ...}`, so that a tool reads the result
instead of parsing text, and knows which format it reads. `ok` is whether
the command did what it was asked, which a tool needs besides the exit
status: `check` exits 1 for errors and still answers. An error that stops a
command is `tondo-error-0.1/1` when JSON was asked, with its status and
whether it is one of usage. `check --json` was an array of diagnostics; it
is now that object, with the array as `diagnostics`, since an array leaves
no room for anything else a command has to say.

The commands, their options and what each does are one table, from which
`--help`, `tondo help` and the completions for bash, zsh and fish are made,
so that they cannot disagree. A mistake shows that command's help, not
every command's.

`tondo graph` reads the imports of a program without compiling it: the
packages, the modules with their files, and what each imports, as text,
JSON, Graphviz or Mermaid. Given a directory or a file, it says which
project holds it and which module it belongs to, which is what an editor
asks of a file it opens; a separate command for that would have answered
less. Tests are left out: they are not part of what the program is made
of.

`tondo test --test-format events` writes a line as each pass begins and
each suite or test starts and ends, and the report last, so that an editor
shows results as they come and still has the report that decides them
after retries. What a test wrote travels with its end rather than as its
own events: the runner learns which test wrote what from the frames that
order a worker's output, which may come after the test ended.

## 2026-10-26 — Tests for editors

An editor lists a project's tests when it opens and after each save, so
`tondo test --list` now stops once the tests are analyzed: it needs their
tree, not their code. The tree is cached with the build's key, which a
build of the same sources reuses.

`--exact` can be given again, so that the tests chosen in an editor run in
one invocation, sharing its build and its workers. Each id must still name
something: an id that names nothing is an empty selection, as a selector
that matches nothing is.

A failed `assert` on a comparison already showed both sides; the failure's
JSON now gives them apart, as `comparison`, so that an editor shows a diff.
What a test writes is an `output` event as soon as the runner knows whose
it is, so a long test's output shows as it runs; its `end` still carries
all of it, so that an event alone says everything about an attempt.

`tondo version --json` lists `features`, so that a client checks for what
it uses rather than for a version number, which a build of `main` does not
change.

The language server already analyzes each project as a build of its
tests, so it knows the tests as they are written: an editor that asks for
them gets `tondo/tests` with a project's tests whenever they change,
without saving and without a process. `--list` stays for a project no file
of which is open. The lenses over `main` came from a pattern in the
extension, which a script without `main` or a `main` in a string fooled;
the server puts them where the parser finds `main`, or a script's first
statement, which also serves editors other than VS Code.

## 2026-10-26 — Tests that run as files change

`tondo test --watch` runs the tests, then after each change those it can
affect, which VS Code's continuous runs use. Which tests a change affects
comes from the same imports as `tondo graph`: a test whose file changed,
or whose module or imports reach a changed module. Anything else, as the
manifest, runs every test, rather than guess. Each run is a whole
invocation in the format asked for, so a client reads a watch as it reads
one run.

A run that an editor cancels is ended with `SIGTERM`, which now stops the
workers as Ctrl-C does instead of leaving them to finish alone.

## 2026-10-26 — Coverage

`tondo test --coverage` counts what the tests run of the project's own
code, not its tests, dependencies or the standard library. The compiler
adds a counter where each statement the program wrote starts, at a loop's
head, at the start of each function, and where each way of an `if`, a `match` or `and` and `or`
starts, in blocks that lowering already makes for each way; a test build
with coverage is a build of its own. Each worker writes its counters when
it ends and `tondo test` adds them up, so the workers stay as they are. The
counts are not atomic: two tasks may lose an increment between them, which
can make a count low but never zero when the code ran, and an atomic add on
every line would slow the tests more than coverage is worth. Counting
where the generated blocks of a line start, as first done, made the line of
a `match` count once more for each arm that ran; counting the statements as
written makes a line's count the times it ran.

A function nothing calls is code that did not run, so a test build now has
every function of the project that is not generic, called or not. The
report is JSON for tools and LCOV for the many that read it; the counters'
layout leaves room for profiling and profile-guided optimization, which
need the same counts of blocks.

## 2026-10-26 — Running programs in VS Code

The extension shows what `tondo` already says as JSON: the programs to
attach to, picked by name from `tondo ps` instead of from every process of
the machine; their counters from `tondo counters`, in a view; a dump's
summary; and the modules of each project from `tondo graph`, in a view and
in Mermaid. A trace opens in Perfetto's web UI, which loads a file only
from a server it allows: the extension serves the timeline once on
127.0.0.1:9001, as Perfetto's own script does, rather than ship a viewer.

## 2026-10-26 — The debugger, part by part

The debugger already showed each binding as text with its structure, as
Tondo would write it: a record with its fields, a variant with its payload,
a collection with its elements. Rather than another kind of glue for every
type, the runtime reads that text back into its parts, so a value opens
field by field and a path names a part: `user.name`, `items[2]`,
`scores["x"]`, in `print`, in conditions, in logpoints and in an editor's
watches. Parts of a value too long to show whole do not open.

A function keeps the addresses of its bindings in a slot of its frame, at
the same place in every call: the first of its probes that runs with a
debugger attached says where, and the frames of any stopped task then show
their bindings, as they were when their line started. A binding that lives
in a register has a copy the probe writes; it is read back after the probe,
so a value the debugger sets stays.

A panic stops where the program's own code panicked, before it unwinds,
with its text: the most useful stop there is, so it is on by default. A
logpoint and a count of hits are the runtime's, so the program does not
stop for each hit. A dump opens in the same adapter, without a program,
at its tasks and stacks.

## 2026-10-26 — The language server, the command line and agents for an IDE

The language server answers what an IDE asks beyond names: who calls a
function and what it calls, from the uses the index already has; the
`impl`s of a trait or a type, and the types of a trait, which the index now
records; the declaration of a binding's type; the parameter each argument
goes to; the structure of a file, from its tokens; and the format of a
range, as the hunks where the canonical format differs. A name that is a
module the file does not import offers the import, as completion and as the
compiler's repair. In Tondo a directory is a module, so renaming one renames
the module: the imports of it and the names it is used by change with it.
The projects of the editor's folders are analyzed when it starts, so their
problems show before any file is opened.

`tondo explain` turns the registry of codes the specifications keep into
help, with the examples of the guide, whose examples of errors already
compile to the code they name; a diagnostic's code links to it. `tondo
new` and `init` make a project with a program and a test, `add` and
`remove` edit the manifest as a person would and fetch at once, so a
mistake shows while the manifest can still go back, `fix` is the MCP's
tool on the command line, and `clean` empties the cache of builds but keeps
what was downloaded unless asked. Agents get the same as tools.


## 2026-10-26 — The extension, from the first day

The extension says where it stands: the status bar has the version of the
`tondo` found, or that it is missing or too old, and a menu of what the
extension does; *New Project* runs `tondo new`, and a walkthrough goes from
installing `tondo` to a project run, debugged and tested. Check and build
tasks read the diagnostics as JSON, rather than match their text: the
Problems panel gets their ranges and codes, and the terminal shows them a
line each. The arguments and environment of programs and tests are
settings, for runs without a `launch.json`; one that has them keeps its
own. A snapshot that differs offers to be updated where its failure
shows, and the examples of Markdown documents are tests, by the line of
their fence, as `tondo doc-test` reports them.

## 2026-10-26 — `tondo setup` installs the extension

Setting Tondo up should leave the editor ready too, with nothing to do by
hand: `tondo setup` installs the extension in each editor of the VS Code
family it finds, through the editor's own command, as it registers the MCP
server through each agent's. The binary does not carry the extension: it
installs the `.vsix` it is given, or downloads the one CI made for the
latest good build of `main` with `gh`, as the install script already
downloads `tondo` itself; without `gh`, the install script makes it with
npm from the checkout. A missing extension does not stop the rest of the
setup, and the line about it says how to get one. The editors' own
commands find them, on `PATH`, or on macOS inside their applications,
which do not put them on `PATH`.

## 2026-10-26 — Benchmarks are tests that measure

The testing specification rules out a keyword for benchmarks, and a
benchmark that is not run with the tests rots. So a benchmark is a `test`
whose own body calls `testing.bench(name, closure)`: the analysis knows it,
as it knows the tree, so `--bench` runs exactly those and the list and the
editor mark them, and a plain `tondo test` runs each closure once, so a
broken benchmark fails like a test. The call is allowed only in a test's
own body, not in a helper, a closure or a suite's setup, so that whether a
test is a benchmark never depends on what happens when it runs.

The compiler turns the call into a loop that asks the runtime how many
runs come next, as Go's `b.Loop` does: the runtime warms up, sizes the
batches so that 50 samples fill the time given, and counts the allocations
of one more batch with an allocation hook of their own, so the samples are
not slowed by counting. What the closure gives goes to an opaque call, so
the optimizer keeps the work that made it. The spread reported is the
median of the distances to the median, which a sample delayed by
something else does not stretch.

## 2026-10-26 — Comparing benchmarks, and a gate for CI

A benchmark compared with numbers measured on another machine, or at
another time on a shared runner, measures the machines. So besides a
baseline saved with `--save-baseline`, `--against <revision>` extracts that
revision from git, once per commit into the cache, builds its tests with
the same compiler, and runs each unit there and then here, one after the
other: both sides share the machine and the moment. That is the comparison
a CI gate should use.

A change is judged by the interval of the change of the median, from a
thousand resamples of each side's samples with a fixed seed, so the same
measures always give the same verdict: slower or faster only when the
whole interval says so and the change is past 2%. `--max-regression`
fails the run on a slower verdict past the percent given, or on more
allocations or bytes per run, which are exact and need no statistics.
Each side is measured in four rounds that alternate which goes first:
a shared machine that speeds up or slows down between two runs otherwise
reads as a change, as when CI on a busy runner judged the same code 68%
slower.

## 2026-10-26 — Reports for people

The JSON report is for tools; people read a run in a pull request, in the
summary of a CI job, or in a browser. `--report markdown=<path>` and
`--report html=<path>` tell any run, not only benchmarks: what failed,
what skipped, what the benchmarks measured and how they compare, and the
coverage. Their form is not normative beyond their sections, so they can
improve without breaking tools. The HTML is one file that loads nothing
and runs no script, so a CI artifact opens anywhere: its charts are SVG,
the samples of each benchmark, its change with its interval, and the
coverage of each file.

## 2026-10-26 — Profiles: pprof, with lines

`tondo trace` already sampled where each thread is every millisecond of
processor time, but named only functions, and only as folded stacks. A
profile is useful when it names the line, and when the tools people
already use read it, so `tondo profile` writes pprof, which `go tool
pprof`, speedscope, Pyroscope and Datadog read, with the function, file
and line of every frame. The compiler already knew where the code of each
statement starts; it now keeps that, compressed as runs of lines, beside
the table of calls of stack traces: less than 1% of a binary, and nothing
to run, so a program in production can be profiled without building it
again. The encoder needs only the standard library, so the runtime can
use it when programs profile themselves.

A profile of processor time does not count memory, which would change what
it measures; a profile of memory does, as sampling memory was always
turned on only for the tools. Besides the samples of live memory, each
stack's samples now add up to what it allocated, which is what to cut
first. `show` credits each line of the program with what the runtime and
the standard library do for it, so the lines it lists are the program's.

## 2026-10-26 — Profiles of tests and benchmarks

A benchmark says how fast; its profile says why, so the two come from the
same run. `tondo test --profile` has each worker sample its threads, or
count its allocations, only while a test runs or a benchmark takes its
samples: the warm-up and the batch that counts allocations are left out,
so the profile is of what was measured. The worker writes what it
collected as the trace channel does, frames named, and `tondo test` makes
each into pprof, named after its test and benchmark. Sampling the threads
records no events, which nobody would read; counting allocations slows
them, so a benchmark measured that way says so in its documentation.

## 2026-10-26 — Profiles of tests and benchmarks

A benchmark says how fast; its profile says why, so the two come from the
same run. `tondo test --profile` has each worker sample its threads, or
count its allocations, only while a test runs or a benchmark takes its
samples: the warm-up and the batch that counts allocations are left out,
so the profile is of what was measured. The worker writes what it
collected as the trace channel does, frames named, and `tondo test` makes
each into pprof, named after its test and benchmark. Sampling the threads
records no events, which nobody would read; counting allocations slows
them, so a benchmark profiled for memory is not compared with one that
was not.

## 2026-10-26 — std.profile and labels

A continuous profiler, as dd-trace-go's, lives in the program: it takes a
profile every so often and sends it. `std.profile` lets a Tondo program do
that, with the same pprof as `tondo profile`, so the encoder moved to the
runtime, which the toolchain includes as a file. In the process the
runtime names its own frames from the executable's table of symbols,
which the loader does not export, and its Rust names it demangles.

Labels divide a profile, by endpoint or by span. A task carries the number
of its set, which the tasks it starts inherit, and a sample keeps that
number, which a signal handler can read; sets live in a table of fixed
size whose slots are taken in turn, each number with its slot's
generation, so labels made per request do not grow without end and a
sample whose set was replaced has none, rather than another's.
`withLabels` takes a body that gives anything, a result included, so a
body that fails is passed on with `?` like any call.

## 2026-10-26 — Profiles where the code is

A profile is read against the code, so the editor puts it there: after a
*Profile*, each hot line of the program ends with its share, its own and
with what it calls, and the flame graph opens beside it. The share credits
each line with what the runtime and the standard library do for it, as
`show` does, so the lines marked are the ones to change. An agent asks the
same with the `profile` tool, which gives each hot line with its code, so
it can optimize without reading a flame graph.

## 2026-10-26 — Signed and notarized on macOS

A `tondo` downloaded with a browser on macOS carries the quarantine
attribute, and Gatekeeper refuses a binary that is not signed with a
Developer ID and notarized. CI's builds of `main` are both: the
certificate goes into a keychain of the runner's own for that job, the
binary is signed with the hardened runtime and a secure timestamp, and
Apple notarizes a zip of it, with an App Store Connect API key, which
needs no account's password. `tondo` makes no executable memory of its
own, so the hardened runtime needs no entitlement. A bare executable
cannot be stapled, so Gatekeeper asks Apple on the first run. Builds of a
pull request have no secrets and are not signed; the script says so.

## 2026-10-26 — Inlining, with every frame kept

Go is fast on small functions because it inlines them, closures included
when the function that calls them is inlined too. Tondo now does the same
in its MIR, in builds that neither step through code, count it nor reload
it: a call of a small function becomes its blocks, with its parameters
the caller's arguments and a loan the place lent, which the MIR's
explicit ownership makes a copy without retains. A call through a `fn`
value the function made itself, a closure or a named function, becomes a
call of its body, the captures read from the value, and is inlined in
turn; the check for cancellation after it goes when the code inlined
cannot wait. Rounds repeat this, so `apply(f, n)` with a closure becomes
one loop.

A trace still names every frame (language 15.7), and a profile gives each
its function, file and line (toolchain 7.15): each call and each run of
code records the call it was inlined at, which records the one around
it, and the runtime expands them, without allocating, into the frames
they were. A heap's site skips the standard library inlined into the
program's code as it skipped its frames.

## 2026-10-26 — mimalloc

Tondo's tasks move between workers, so the memory one task allocates is
often freed on another thread. glibc's allocator makes that free wait for
the lock of the arena the block came from, and the benchmark that spawns
a thousand tasks spent most of its time there. mimalloc, under the
allocator that counts budgets and samples, gives each thread a heap of
its own and sends a block freed elsewhere back to it without a lock. It is
a dependency of C, as `ring` already is, and smaller than an allocator of
our own would be.

## 2026-10-26 — Strings grow in place

`x = "{x}..."` builds a new string and drops the old one, which makes a
string built in a loop cost the square of its length, and `format.Builder`
kept its pieces apart to avoid that. When nothing else holds the string,
which its count says, the text now goes on in the string's own block,
which mimalloc says how much it can hold, and the block grows by doubling
when it must; a string another binding holds is copied as before, so no
program can tell. The rest of the text must be of types whose `Display`
runs no code of the program, and must not read `x` again, whose bytes
would move under it. `format.Builder` is a string grown so, and a string
literal, which counts no references, is retained and released no more.

## 2026-10-26 — Cranelift, patched

Every `+` and `-` of integers is checked, and Cranelift 0.137 branches on
the overflow of a signed one with `seto`, `test` and a jump, where it uses
one `jo` for unsigned additions and for multiplications: arithmetic loops
paid twice the instructions Go's do. Four rules of its instruction
selection make the signed ones one instruction too, on x86-64 and arm64.
The maintainer chose to keep the patched crate in `vendor/`, which
`[patch.crates-io]` puts in place, over waiting for Cranelift to take the
rules: `vendor/README.md` says what changed and how to update it. On
2026-10-27 a third change made a multiplication by 3, 5, 7 or 9, as a
remainder by a constant has, a shift and an addition, as Go makes it:
`i * i % 7` over a million integers went from 849 to 816 µs, Go's 820.

## 2026-10-26 — A notified task runs next

A task that wakes another, as a sender wakes a receiver or a task that
ends wakes the one that waits for it, now leaves it next to its worker,
which runs it as soon as the waker waits or ends, as Go's `runnext` does:
two tasks that hand values to each other run one after the other on one
thread, without crossing cores or waking a sleeping one, and a channel
benchmark runs faster than Go's. A task that waits there longer than
20 microseconds, because its worker runs on, is taken by an idle worker,
which is woken for it at most once every 100 microseconds, so stages of
a pipeline still run in parallel; and the queue goes first now and then,
so two tasks that wake each other do not hold back the others. A task
that a task starts goes to its worker's own queue, which an idle worker
takes half of when it finds nothing else, so spawning many tasks no
longer takes the queue's lock for each.

## 2026-10-27 — One worker looks for tasks at a time

Tasks that a task starts wait 20 microseconds in their worker's own queue
before an idle worker takes half of them, as a task next does: short
ones end on the worker that started them, in the caches where they were
made, and long ones are shared out about as soon. Only one woken worker
looks for tasks at a time, as Go wakes a thread only when none spins: it
looks again after the 20 microseconds if what it found was too young,
and once it finds a task it wakes the next if more wait. Before, a loop
that started a thousand tasks woke a worker a hundred times and had them
stolen three at a time, by workers that read the lines its worker wrote
at each start, and that worker lost more than the others gained; a round
of a thousand short tasks takes a third less on 32 cores now, and long
tasks are shared out twice as fast, as the woken worker no longer gives
up its turn as soon as it wakes. Idle workers also look at the others'
queues without the queue's lock, which they took in turns.

## 2026-10-27 — The compiler proves indexes and sums

A pass over each function's MIR, `prove`, after inlining and outside
debug and live builds, proves what Go's `prove` pass does: that `xs[i]`
is within `xs`, and that a sum, a difference or a product of `Int`s
cannot overflow. It keeps bounds on differences, `a - b <= k`, between
integer locals, zero and the lengths of array locals, closed under
transitivity: the condition of a branch adds one on each way, `x = y + c`
moves them, and at a loop's head those a turn does not keep are dropped.
Anything that may change a length forgets it: a mutation, an assignment,
a loan, or the array passed whole to a call that may borrow it. A read
or write it proves becomes `InBounds`, and an operation `Exact`, which
codegen compiles without their checks; it follows only the locals that
index arrays or take part in arithmetic, and what those come from or are
compared with, so that compiling costs no more. `for x in xs` over an
array or a deque needs no proof: it walks a snapshot that nothing else
names, and reads each element without a check but in live builds. Summing
10,000 integers by index went from 6.2 to 2.5 µs, with a panic's values
passed to the runtime in memory rather than in the registers of a call,
which a loop's own values had been moved around for.

## 2026-10-27 — Short counted loops run four turns at a time

A loop over a range of `Int`s whose body is short, 24 statements or
fewer, and holds no loop runs its body four times in a row while four
turns remain, and the last turns as before. A bound `last - 3` is
computed once before it, when it fits, so a turn of four tests the range
once and jumps once; `continue` and `break` in a copy go to the loop's
own step and exit, so each turn means what it did. It runs after
inlining and before `prove`, which then sees each copy's index under the
length that bounds the loop, and only where `prove` does. A closure
applied a thousand times took 36% less, and summing integers by index
10%. Cranelift does not unroll, nor does Go's compiler.

A loop over an array's elements, `for item in items`, is the same counted loop, whose first
statement reads the element at the counter, known to be within the array, rather than the counter
itself; it runs four turns at a time too since 2026-11-11, which `sum` and a program's own loop
over 10,000 integers took in half the instructions and half the time, and `map filter sum` in 21%
fewer. A loop over elements whose turns call the runtime, to count a value or measure a text, as
most loops of the standard library do, is not: it gained nothing, and four copies of each made
compiling a large program cost 8 to 10% more instructions, where without them it costs the same.
A square root, a floor or a rounding, which the processor does in one instruction, calls nothing,
and a loop of them runs four turns at a time too.

## 2026-10-27 — What inlining hands around is not made twice

Inlining leaves a call's result in the callee's result local, copied into
the caller's, and a test of what the caller got right after the callee
decided it: `math.sqrt(x).unwrapOr(0.0)` made a `Result` in memory,
copied it twice, and read its tag back. After inlining, a copy of a value
read nowhere after takes the value directly (`elide`), and a short block
that ends in a test is copied into each way into it (`thread`), where
what the way knows folds the test: the variant an enum local was given,
a constant, or the local a value was copied from; a known variant's
payload is read from where it came from, and a value read by nothing is
not made. A binding that borrows, and what it borrows, are never known
and always written. Copies keep the scope of inlining of their block, so
that traces still name each function, and are merged into the block
before them only within one scope.

## 2026-10-27 — A caller makes its callee's first test

A function that starts with a test of its parameters that may return at
once, as `if n < 2 { return n }` or a `match` whose first arm makes a
value, has that test and that way out copied into each caller that still
calls it after inlining, as GCC splits a function's head from its body:
the caller calls only when the test does not return. Half the calls of a
recursion end in its base case, and each paid for a frame, the registers
it saves and restores, for a comparison; Cranelift does not delay saving
them past such a test, as LLVM does. The test and its way out hold four
statements at most, each of which only computes, with no read that may
panic, so that making the test again in the callee changes nothing. The
call's arguments are read by both, and the result goes where the call's
did. `fib(35)` went from 52 to 29 ms, and Go's takes 37.

## 2026-10-27 — A source finds a record's keys among its fields

Decoding a JSON object into a record made a string for each key, and an
event, with its own string, for the first token of each member the record
has no field for: in a document with ten members and five fields, more
than half of the time. `Source` has a new method, `nextField(names)`,
with a default that does what derived decoders did: it gives the index of
the next member's key among the record's keys, and skips, or rejects,
the members of none of them, failing at the same position as before.
JSON's reader compares the key in place and skips an unknown member
without events; it also reads every string and number into one buffer it
keeps, and the keys of the objects open into another, rather than a
string each. A record's derived decoder matches the index, not the key.
The maintainer chose this over a format-specific decoder: other formats
keep working through the default. Decoding the benchmark's ten-member
object went from 3.05 to 1.58 µs, and from 52 to 13 allocations; Go's
takes 1.28 µs.

## 2026-10-27 — A string knows its room

A string appended to in place, as `x = "{x}{y}"` and `format.Builder`
make one, asked the allocator each time how much its block held. The
header of a string is now 24 bytes, not 16: its count, its length and
its room, which an append checks first and asks the allocator about only
once that room is used. The maintainer chose the 8 bytes more for every
string, which the specification leaves to the implementation, over a
call of the allocator for each piece appended. Pieces of up to 16 bytes,
as most are, are also copied with two moves of a word rather than a call
of `memcpy`, for a new string, an interpolation and an append. A hundred
appends of a word and a number went from 1.40 to 0.89 µs, and an
interpolation from 29.8 to 23.6 ns. A text of up to 16 bytes known when
compiling, as a literal a function is called with, is then appended by
the compiled code itself, as an element is pushed, when the string is
held only there and has the room: 0.71 µs, under Go's 0.76.

## 2026-10-27 — An array of constants is made once

An array literal whose elements are all constants, integers, floats,
booleans, characters or strings, was made anew each time it ran, a
constant's at each use, as the field names a derived decoder hands its
source. It is now made once, in the program's data, with a count of
2^62 that no release brings to one: changing it, or a copy of it, copies
it first, as for any array that is shared, so each run of the literal
still sees the value it wrote. The count is written as the array is
shared, so the data is writable. A live build makes the array each
time still: a reload that converts an array replaces and frees it. A
million calls that walk a constant array of three names went from a
million allocations to none.

## 2026-10-27 — What an owner gives up moves rather than being counted

After inlining, a value read out of a local that owns it, a payload a
match binds or the result a `?` unwraps, is retained for where it goes,
and the local is released later without that part having been read
again: three atomic operations for a string decoded from an event,
where moving it takes none. A pass over the MIR after inlining, `moves`,
turns such a retain into the zeroing of the part in the local, as the
lowering already does for a binding a match takes by value: the copy
keeps the local's reference, and the local releases the rest. It does
so only when the local is the function's own, not a parameter, its
result or what a binding borrows or lends; when the part is reached
through fields and variants only; and when on every way on the local is
released, whole or by a part that holds this one, before anything reads
that part or the function returns. A whole local moved and then released
loses both statements. Decoding the benchmark's object went from 1.54
to 1.38 µs, parsing one from 3.1 to 2.4 µs, and a thousand numbers from
85 to 69 µs.

## 2026-10-27 — The editor says what became of each change to a live build

Saving a file under Debug Live showed, for five seconds, which functions
changed, and a change the program could not take in came as an error,
the same as one that did not compile; Run Live said it only in its
terminal. Nobody could tell, after a save, whether the program ran the
new code or had to start again. The extension now reloads both itself:
Debug Live through the debugger's `reload`, whose refusals are answers
with `message` and `restart` as in `tondo reload --json`, and Run Live,
which runs `tondo run --live` rather than `--watch`, through `tondo
reload <pid> --json` when a file is saved or changes on disk. A status
bar item stays while the program runs and says what became of the last
change: applied; applied with frames that finish on the old code; not
applied, the files not compiling, with the program on its old code; or
needing a restart, which Run Live does by itself, as `--watch` does, and
Debug Live offers. The debugger had turned the program's own refusal
into a failure, so that a change needing a restart read as a broken
reload: it now reads the program's answer as `tondo debug` does.

## 2026-10-27 — An executable for macOS names its own functions

A profile of a program for macOS named the program's code, which the
compiler's table describes, and nothing else: the runner's frames under a
test, the runtime's and the C library's showed as `tests+0x57cb0`. Two
things were missing. `tondo` read the executable's symbols from address 0,
where `object` puts a Mach-O file's base, while the runtime gives offsets
from the Mach header, which the `__TEXT` segment loads at 0x1_0000_0000;
the base is now the address of the segment that maps the file's start, for
ELF as well. And the linker wrote only the symbols other files can see,
while the runtime's and the compiler's functions are local: it now writes
the local symbols of code too, unless it strips, so that `tondo`, the
runtime's own reading for `std.profile`, and debuggers find them, as the
ELF linker always did. The flame graph's page takes the editor's colors in
VS Code, or the system's light or dark, and its width, with Tondo's code
in warm colors and the rest in grey.

## 2026-10-27 — A test fuzzes by calling `testing.fuzz`, with typed inputs kept as JSON

Fuzzing follows benchmarks: a test that calls `testing.fuzz(name, seeds,
body)` in its own body fuzzes, which the analysis knows, rather than a
keyword or an annotation. Its input is a value of any type with `Encode`
and `Decode`, which the compiler writes for records, enums and newtypes,
not raw bytes as libFuzzer takes: seeds are written as values in the code,
and every input is kept as the JSON `json.encode` writes, so that a failure
reads and edits like a snapshot. Without `--fuzz` the body runs with each
seed and each input that failed before, from the `fuzz/` store beside
`snapshots/`, committed: a failure found once is a test of regression with
nothing random. The search of `--fuzz` mutates inputs knowing their type's
shape, is guided by the counters of `--coverage` over the project and its
dependencies and by the operands of its comparisons, reduces what fails,
and runs until Ctrl-C or `--fuzz-time`; an `err` of the body is a failure,
like an `assert`. The loop is written in Tondo, `std/fuzz.to`, which the
compiler calls with the input's type and the body's error, inferred as for
`testing.bench`; the runtime gives it the inputs.

## 2026-10-27 — A response's text is read in the charset it names

`Response.text()` read every body as UTF-8, and failed on the pages that
servers still send as ISO-8859-1, as google.es does. It now reads the
`charset` of the content type, and `lines()` and `Request.text()` with it:
UTF-8 when it names none, and Windows-1252 for that name and for the labels
of Latin-1 and ASCII, as the WHATWG Encoding standard has browsers read
them, which is a table of 32 characters. Other charsets, as Shift_JIS or
UTF-16, would need the large tables of a library of encodings for bodies
that are rare now: a body in one fails with an error that names it and
says to read its bytes. The body is still read once, but a second read,
which gave nothing and said nothing, now fails and says to keep what the
first read gave.

## 2026-10-27 — A `Byte` encodes as an integer from 0 to 255

`Byte` had no `Encode` or `Decode`, so neither had an `Array[Byte]` nor a
record with one, and a fuzz test of a binary parser had to take a
`serialization.Bytes`. A byte is now an integer from 0 to 255 in every
format, so an `Array[Byte]` is a list of them, as JSON readers of other
languages write it, and one out of range fails as `Range`;
`serialization.Bytes` stays the type for bytes that a format holds as its
own, natively or as base64. The fuzzer knows the shape: it makes and changes
a list of bytes as bytes, with flipped bits and the values at their edges.

## 2026-10-27 — `main` infers its errors

A `main` that used `?` had to declare the union of everything it
propagated, as `!(serialization.Error | process.ProcessError |
text.TextError)`, and import modules only to name those errors. The
implicit `main` of a script and the body of a test already inferred that
union, because no one imports their signature, and neither does anyone
import `main`'s: `fn main()` now infers it too, and a declared error stays
what it was. So that nothing sees the signature before its body is
checked, nothing calls `main` or takes it as a value (`E1803`). Every other
function declares its errors, which are its interface; when one propagates
an error it does not declare, a single `E1301` names all it propagates, and
its repair writes the result and the imports those errors need.

## 2026-10-27 — The toolchain writes a file's imports

Every module a file uses is imported, the standard library's too: the
imports say at a glance what a file touches, to a reader and to an agent,
and modules are not names that a variable called `text` or `json` could
hide. Writing them is the toolchain's work instead, as `goimports` does for
Go. A name of a module the file does not import is `E1001` with a safe
repair that adds the import where the formatter would sort it, and an
unused import is `W1001` with one that removes its line; the arguments of a
call of an unknown name are checked for unknown names too, so that one pass
finds every import missing. The language server's `source.organizeImports`
applies those repairs, which the VS Code extension runs on save, and
`tondo fix`, and the tool `fix` of agents, apply them with the rest of the
safe repairs, inserting where another repair removes.

## 2026-10-27 — `App.files` lists a folder when asked

A folder served with `App.files` answered `404` at its own path, so a user
could fetch a file only by knowing its name. `listing: true` answers a
folder's path with an HTML page of its entries, folders first, each file
with its size and a link that downloads it, and a link to the parent but at
the top. It is off by default, because a list of the files a server keeps
is not something every server should show; the page is plain HTML with
absolute links, so that it works at a folder's path with or without its
final `/`.

## 2026-10-27 — A union shows the member it holds

A union had no `Display`, and one could not be written for it: neither a
union nor its members are the module's own, so `impl Display for Scalar` of
`alias Scalar = Int | Float | Bool | String` broke the coherence rule. Only a
newtype could take one, at the cost of wrapping each value. A union now has
`Display` when each of its members does, as arrays, options, maps and tuples
have when their parts do, and shows the member it holds. That lets a map of
plain values of several types, as the data a panic carries, be shown without
a `match`; an error names the members that lack it.

## 2026-10-27 — A panic has a code, a message and data, and a JSON form

A panic said only a message under `P0008`, so code that wanted to tell one
panic from another, a log aggregator or an agent read text. `panic` now
takes the program's own code, dotted camelCase words that cannot be the
language's `P` codes, and data, a map whose values are `Int`, `Float`,
`Bool` or `String`, rather than `Any`, which Tondo does not have and which
would make a panic carry anything at all: a panic always has the same
shape. The report shows the data the program gave; the language's panics
carry their values too, an index and a length, without repeating them. A
literal code that is not one is a compile error, a computed one `P0017`.
`TONDO_PANIC_FORMAT=json` makes a program write how it failed, a panic,
`main`'s error or a fault, as one line of `tondo-panic-0.1/1`, which the MCP
tool `run` sets and returns as data and which a test's failure shares; the
panic's data come to `tondo test` in a frame of their own.

## 2026-10-27 — `recover` stops a panic, by returning

A panic ended the program, so a server could not survive a bug in one
handler, and a tool that runs plugins or jobs could not report one and go
on. Go's `recover()` in a deferred function, and `try`/`catch`, are
ceremony and catch by type; `recover { ... }` is an expression of type
`T ! Panic` instead, handled as any result, with the panic's code, message,
data, place and stack, and `p.resume()` to let it go on. It stops what the
block runs: its code, what it calls, and the tasks it starts without
`detached`, the first of whose panics stops the block's task as soon as it
can, at the latest at its next wait, so that a worker that panics cannot
leave the block waiting forever. A deadlock, a crash, a panic in code that
C called and the test runner's `skip` and `failNow` are never stopped.

It unwinds without tables of unwinding and without walking the stack.
In a program with a `recover`, a statement that may panic is followed by a
check of one global word, the reasons the runtime has to be asked
anything, which is zero but while something happens; a panic records itself
in its task and returns, and the check leads to the frame's cleanups, which
release what the frame holds and run its `defer`s, and then to the caller,
or to the `recover`. A panic the compiler raises inline jumps straight
there. Programs without a `recover` compile as before; with one, the checks
cost about 7% on a loop of calls and 2% of size. `std.sync` locks are let
go rather than poisoned, as the cleanups run, and `std.http` answers a
handler's panic with `500`, reporting it with the request's method and path.

## 2026-10-27 — A `recover` costs nothing where nothing panics

The first cut checked for a panic after every statement that may raise
one, and each check split the code into blocks that the optimizer counted
as cost: a program with a `recover`, as every server of `std.http`, lost
inlining, unrolling and the closures it calls directly, up to four times
slower on a loop of closure calls. The optimizer now counts only the code
that runs when nothing panics, and a call it inlines unwinds straight to
its caller's cleanups, with no check after it. A call it does not inline is
a `try_call` of Cranelift: a function that unwinds makes its frame's return
address the handler of the call that called it, which a table beside the
stack traces' gives, so a call that returns costs nothing more; a
two-instruction entry of the runtime reads the frame record, so leaf
functions keep going without frame pointers. Arithmetic of floats and the
operations that only add or take an element need no check, nor do those
the optimizations proved cannot panic. The benchmarks of `bench/` run
within their noise of the same programs without a `recover`.

## 2026-10-28 — A context goes with the work that runs

A server needs values that belong to the work it does, such as the
request, the user or the trace, in every function below the one that knows
them. Go passes a `context.Context` to each, which is ceremony. Odin passes
a hidden context to each call, with fixed fields. .NET's `AsyncLocal` needs
global state to declare. A Tondo context is declared as a record with
constant initial values and read by its name: `Trace.span`. A `with`
statement changes it until its block ends, restored whichever way the block
ends, so three contexts are three lines and never three levels of nesting.
A change never flows back to a caller, and a task takes the context of the
point where it is spawned, as .NET does. A value set at the start of `main`
is the program's, which covers what a mutable global would hold.

Each task points to a table of `Ref`s, one per context the program
declares, which nothing changes once it is made. A `with` makes a new table
and keeps the old one to restore it, and a spawned task shares its
parent's. Reading a field loads it from its `Ref` in place, and the table
of initial values is made when a task first reads one, from a function the
compiler writes for each context.

## 2026-10-28 — A deadline unwinds its block, as a panic would

`within limit { ... }` needs to stop a block in the middle of a wait and
leave it the way a cancellation leaves a task, with its `defer`s run and
its tasks cancelled, but only the block. It uses the unwinding of
`recover`: a wait arms a timer at the task's deadline, which ends the wait
as a cancellation does and marks the task, and the task then unwinds to the
`within` that set the deadline. A `recover` and a `within` each ask, where
they catch, whether what unwinds is theirs; what is not, a deadline at a
`recover` or a panic at a `within`, goes on to the next one out. Only the
task that runs the block keeps the deadline; the tasks it starts read it
and are cancelled with the block. `spawn detached` now tells the runtime
before the task starts, so that a detached task never belongs to a
`recover` or a deadline, even for the moment before its `detach`.

## 2026-10-28 — What a context shows, the compiler writes

A field marked `@log` goes in every log event and panic report, and one
marked `@label` in profiles, while it differs from its initial value. The
runtime cannot show a value by itself, and an option should show what it
holds rather than `some(...)`. So for each context with marked fields, the
compiler writes a function in Tondo, checked as any other, that gives each
field's name and `Display` text when it differs from its initial value, or
when an option is `some`; the runtime calls it where it needs them. The
labels of a task are those of its contexts with those of `withLabels` over
them, made again whenever a `with` changes the contexts, so that a sample
reads them as it read the labels before.

## 2026-10-28 — Every number has every width and every operation

Porting dd-trace-go showed that only `Int` and `Float` were complete: a
`UInt64` could not be encoded, parsed in a base, multiplied modulo 2^64 or
rotated, and a trace's 128-bit id had no type. Every integer of 8 to 128 bits,
signed and unsigned, and both floats now get the same operations (language
18.9): the overflow variants 18.2 promised, the bits, the bytes, text in any
base and the named conversions. `UInt` is the unsigned `Int`, shown as such
as `Int` is shown for `Int64`. The operations are intrinsics rather than
some three hundred functions of the prelude: each is the machine
instruction Cranelift has for it, as Go's `math/bits` are, and what
Cranelift does not do for 128 bits, dividing, telling the overflow of a
product and converting with floats, the runtime does. An integer of 128 bits
is aligned to 8 bytes in Tondo's own memory and to 16 in C's.

`Additive` (`+`, `-`, `sum()`) and `Number`, which requires it and adds `*`,
`/` and literals, let generic code compute and let a program's types take
the arithmetic operators, with their arithmetic meaning only: a money amount
or a duration adds without inventing a product, a decimal does all four. A
trait may now require others, which is what `Number` needs; it receives
none of their methods. `Integer` is closed over the ten integers and gives
generic code everything a concrete integer has.

## 2026-10-28 — An integer past `Int` travels as its digits

The serialization events carry an `Int` or a number's digits, and the
formats had turned digits that did not fit into a float, so a `UInt64`
trace id lost its last digits. Every integer now writes itself as an `Int`
when it is one and as its digits otherwise, and each format writes the
digits as its own integer: JSON and YAML as they are, CBOR as a major type
or past 64 bits as a bignum tag, MessagePack up to 64 bits unsigned, and
TOML up to its signed 64 bits; what a format cannot hold fails instead of
rounding. The impls of every width are one line each over two functions
generic on `Integer`, which is why `Integer` also requires `Ordered` and
`Display`. A `Float32` writes its shortest digits, so that JSON says
`0.1` and not the `Float` it widens to.

## 2026-10-28 — Annotations are records the program declares

Gin binds and validates a request through tags on fields, and a router reads
the routes of its handlers; Tondo had only its own closed annotations. A
program now declares an annotation as a record with defaults, the form of a
context, and writes it with its type's capital, `@binding.Check(min: 3)`, so
that it resolves, imports and completes like any type, and its arguments
are checked as a literal of the record: a misspelled field is an error that
names the fields, not a tag that silently does nothing. The language's own
stay lowercase. Annotations change nothing about what they annotate.

Reflection reads them with their types: the description the compiler writes
for a type asks for the values of its annotations, and the checker puts them
there, so a program pays for them only where it reflects. `meta` reads them
before the program is checked, so the compiler evaluates their constants
itself, and a `const` whose value needs the checker is an error that asks for
the value. A provider cannot import the program's modules, so it decodes an
annotation into a record of its own with the fields it reads. A generator's
root can be a public function, which is how a library writes a router; and
a method takes its own type arguments, `info.annotation[Check]()`, which the
grammar always allowed and the checker now does.

## 2026-10-29 — Embedded files are constants of the program

Go embeds templates, pages and data with `//go:embed`, and Gin and Hugo serve
them from memory; Tondo needed files next to the executable. `embed.text`,
`embed.bytes` and `embed.files` of `std.embed` read files of the package
when the program compiles: a call is a constant, so it goes in a `const` or
any expression, rather than a new kind of declaration, a table of the
manifest or an annotation. The path is a literal relative to the file that
writes it, as Rust's `include_str!`, and cannot leave the package; hidden
files of a directory are left out, as Go does, so `.env` does not ship.

The bytes become a static array of the program's data, as constant arrays
already are, so nothing is copied when the program starts or uses them; an
`embed.Files` is two sorted arrays and a digest of each file, which
`app.embedded` serves as its `ETag`. What a build reads must be known before
it compiles, for the caches of `run` and `test` and for `--watch`, so the
toolchain finds the calls in the sources' tokens, as `tondo graph` finds
imports, instead of asking the checker. A provider or generator does not
embed: what it reads comes as an input.

## 2026-10-29 — A program stops in order when it is asked to

Ctrl-C, `kill` and `docker stop` ended a Tondo program at once, without its
`defer`s, as they end a Go program that does not catch them; a server in
Kubernetes needs to stop accepting, finish what it does and close what it
holds, and in Go every program writes that. `SIGINT` and `SIGTERM` now
cancel the program as `job.cancel()` does, which the runtime already did for
a panic: every wait ends, the tasks run their `defer`s, and it ends with
`128 + signal`; a second one ends it at once, for a program that computes
without waiting. A handler only writes the signal to a pipe, which a thread
reads, so the program's code never runs in a handler. `process.signals`
gives a channel of the signals a program wants instead, which then no longer
end it; two equal ones not read yet are one, as the system's are.

The rest of the first lot of what porting dd-trace-go needs follows the
same rule of doing what the system would otherwise make every program write:
the route that answers a request is chosen before the middleware runs, so
that it sees the route's template; a Unix socket's address is an `Address`
with a path, so the operations of TCP and UDP take it unchanged; the hashes
that are not for security are written in Tondo over the numbers' named
operations, and give integers, which is what they are used as.

## 2026-10-29 — The web's pieces, as Go's and as a browser's

The second lot for porting Gin and Hugo. URLs follow RFC 3986, as Go's
`net/url` does, rather than the WHATWG standard browsers follow: a link in
a page is a relative reference, which WHATWG cannot hold without a base. The
`Url` is a record of decoded parts that `with` changes and `Display` encodes
back, and resolving is written in Tondo after the RFC's own algorithm, which
its examples test. Cookies are encoded with `urlEncode` as Gin does, so any
text fits, and a client keeps them only when asked to, since a program that
calls several services rarely wants one's cookies sent to another.

`request.clientIp()` believes `X-Forwarded-For` only from the networks an
app declares as its proxies, once for the app as Gin's `SetTrustedProxies`
does, but trusting none by default where Gin trusts all: a client can write
the header, and a rate limit or an audit log keyed by a forged address is a
hole. `std.regex` now takes the flags `i`, `m`, `s` and `U` in a pattern and
replaces by a function, which 0.1 had refused: Hugo passes its users'
patterns, written for Go, and the function cannot fail, so the cost the
refusal guarded stays visible.


## 2026-10-29 — Under emulation each night

CI took about 50 minutes a push again, bound by the tests under qemu's
emulation of x86-64 and by macOS on Intel, the slowest runner. In 120 runs
the tests under emulation failed alone ten times: three were races of the
runtime that hardware has too and emulation's slowness brought out, seven
were tests whose time limits were too tight for it. A race that shows once
in hundreds of runs is found as well by a run each night as by one each
push, so they run each night now, with clippy and the cases' other builds
on Intel, which arm64 and Linux already run on each push. Each push still
builds `tondo` across for the other architecture and builds for it with
the pack the emulated `tondo` writes, which nothing else tests, and the
musl jobs keep their builds between runs as the others do.

## 2026-10-29 — Zones from the system, names from CLDR

The third lot for porting Hugo: walks, globs, watching, compression and
time zones. A zone comes from the system's tz database, which the system
keeps current, and one it lacks from the copy jiff bundles, which a
container with no database still has; `TONDO_ZONES=builtin` takes every
zone from that copy, for the same answer on every machine. A
`ZonedDateTime` is a type apart from `DateTime`, which stays UTC: records
are equal by their fields, so a `DateTime` that could hold any zone would
make two names of one moment unequal, as Go's `==` on `time.Time` does.
Patterns are `strftime`'s rather than Go's reference date, as C, Python,
Rust and Ruby write them; the names of months and days come from CLDR in
the language a call names, never from the system's locale, which musl and
most containers do not have. Globs take `**` and braces, as people write
them, and every text is a pattern, so `path.matches` cannot fail.
`fs.watch` sends a batch once changes settle, each path once, telling a
file replaced through a rename from a new one by the paths it saw exist;
it ends when no copy of its receiver is left. Zstandard is ruzstd's, pure
Rust at its fastest level, so the runtime builds for every target without
C.

What can be Tondo is Tondo, so that what makes compiled code faster makes
the library faster too: paths, globs, ISO 8601, `strftime` patterns and
durations are written in it. The runtime keeps what Tondo cannot do or a
crate does better: the system's calls, zone rules, CLDR's names and
compression. The checker keeps its own list of the directives, to reject a
wrong literal pattern while compiling; a case uses every directive, so the
two lists cannot drift apart unnoticed. A stream that writes into another,
as gzip's, closes itself with `close()` and lets go of the other, which
closes with its last copy, as the writers of JSON, CSV and logs already
did: closing it outright would close the console under
`gzip.writer(console.stdout())`.

## 2026-10-29 — A producer learns that nobody listens

Only senders close a channel: closing says no more values come, which only
a producer knows, and a consumer that closed would make the producers'
`send` panic. So a producer could not tell that every receiver had gone,
and a task a library starts with nobody holding its `Join`, as `fs.watch`'s,
went on sending to nobody. `sender.isAbandoned()` now says that no copy of
the receiver is left. What a send does then is chosen when the channel is
made, since a program may want either: by default it keeps the value or
waits, as before, and `discardWhenAbandoned: true` drops it at once, a send
that was waiting for room included.

## 2026-10-29 — Streams across tasks, hooks around requests

The fourth lot, for Gin and dd-trace. A stream the program writes is a task
that reads a pipe: the runtime calls the program's code only when a task
starts or a `defer` runs, never in the middle of a write, and a writer made
of functions run by a task behind it would report a function's error in a
later write, which would mislead. `io.pipe` writes at once, with no buffer
but its capacity, so a reply streamed through it reaches the client as it is
written. A request's host and scheme believe `X-Forwarded-Host` and
`X-Forwarded-Proto` only from the proxies an app trusts, as `clientIp` does.
A client's hooks have the shape of a server's middleware, one function
around `next`, so a tracer times a call and sees its error where it is.

## 2026-10-29 — What a tracer reads of the runtime

The fifth lot, for dd-trace. The program reads the counters `tondo counters`
shows as a list, so a counter the runtime gains reaches it without a new
field, and each goes to a metrics service as it is. The runtime counts only
when something watches it, so that a program nobody watches pays nothing;
a program that reads its metrics counts from its start, since the compiler
sees the call, rather than from the first, which would miss what came
before. The profile of contention attributes a lock's waits to where it was
let go, as Go's mutex profile does, which points at the code that holds it
too long; the profile of blocking has the waiters. Both sample a wait for
each 100 µs waited on average and every longer one, so many short waits do
not each pay for a stack. `setLabels` gives back the labels it replaced, for
a span that starts and ends in different calls.

## 2026-10-29 — Formats in interpolation, normalization and XML

The first half of the sixth lot, for Hugo. An interpolation takes a format
after `:`, Python's and Rust's, since every author knows it: a program
pads, aligns and rounds where it writes the text, instead of calling
`padStart` and `toFixed`. The format is literal, so the compiler checks it
against the value's type and says what that type takes; braces that only
look like one, as `{color: red}`, stay text, but those that mean one and do
not read, as Python's `{price:.2f}`, are an error that shows how to write
it. The formats are functions of the prelude in Tondo; only the digits of a
float in scientific notation come from the runtime, which rounds them
exactly.

Unicode's normalization is in `std.text`, written in Tondo over the
`unicode-normalization` crate, whose tables are data the runtime keeps, as
it keeps the zones'.

XML goes through `std.serialization`, as every format does, so a type that
reads JSON reads XML. What XML needs and no other format has, attributes,
text and element names, comes from annotations of `std.xml`, which the
code the compiler writes passes to the format as notes, before a record's
map and a field's key: a format reads its own notes and ignores the rest,
so an XML annotation leaves the type's JSON alone, and Protobuf's field
numbers can come the same way. XML has no types, so its scalars are a new
event, `Untyped`, that every scalar reads from its text; the formats that
have types never emit it, and stay as strict as they were. Reading builds
the tree first and walks it, which lets elements of one name after another
become a list only when the type asks for one, and text alone become a
record when the type is one. No DTD is read, so no document reaches a file
or a URL, or grows by expanding entities.

## 2026-10-30 — Templates written in Tondo

The second half of the sixth lot, for Hugo and for the sites a program
serves. A template's actions are Tondo's expressions, not Go's language of
dots, pipes and `printf`: an author writes what they already write in code,
`{{ post.title.toUpper() }}` and `{{ price:.2 }}`, and a condition is a
`Bool`, as everywhere else. Go is where porting starts, so the delimiters,
`{{-`, the blocks and the escaping by context are Go's, and the guide gives
the equivalent of each of its forms. A line that holds only blocks is left
out, which spares templates the `{{-` that Go's need everywhere.

A template whose text the compiler sees is compiled into the program, and
not interpreted: its expressions are linked and checked as code, with the
type of its data, so a misspelt field is a compile error that lists the
fields, and its `render` cannot fail. It runs as fast as the code it is.
Before the program is linked, its templates become statements whose leaves
are two nodes, a value written in its place and another template written,
so that `if`, `for` and the bindings are the language's own. A template's
names are its data's fields, so its closure captures nothing of the
function that makes it. A `Set`, for templates that arrive while the
program runs, sees its data by reflection, with the names the code uses,
and not through `Encode`, whose `@name` serves formats. One parser, in
Rust, reads both kinds, for the compiler and for the runtime, which gives
`std/template.to` the parts of a template to interpret; their escaping,
their formats and the code of a `Set` are Tondo.

## 2026-10-30 — Commands and MCP servers from the program's types

The seventh lot. A command line is read into a type, as clap does with its
derive, and not built with calls, as cobra does: the record is what the
program uses, so it is the description too, typed, and its `///` the help,
which cannot fall out of step with the code. An enum is the subcommands. Both
are read by reflection, which now gives the documentation of types,
variants and methods as it gave that of fields. A default is a text, as
written on the command line, since an annotation cannot hold a value of each
field's type.

Styles are Spectre's markup, blocks that return to the style before, rather
than functions that wrap a text, whose resets do not nest. Since the
compiler sees which parts of an interpolated markup are the program's and
which are values, it escapes the values and checks the styles, which spares
Spectre's `Markup.Escape` and its errors at run time. Everything a terminal
shows goes through `console`, as in Spectre, and only errors go to standard
error; a bar or a question off a terminal writes nothing that a pipe would
mind.

An MCP server is a value whose `pub` methods are its tools, as the helpers of
a `template.Set` are its functions: one idea for both. A tool with one record
parameter takes the record's fields as its arguments, which is how their
documentation reaches the schema a model reads. While `mcp.serve` serves,
the console writes to standard error, since a `println` on the protocol's
stream would break it. For the error of a tool to be its `Display`,
reflection shows a value whose type implements `Display` in its file as it
displays, which a `Set` writes too.

## 2026-10-30 — Numbers for people, and what a program was built from

The eighth lot. Numbers, percents, money and plural forms follow CLDR, the
data the dates already carry, and never the system's: musl and containers
have none, and a program should write the same text on every machine. The
data is a table the runtime keeps compressed; the patterns, the rounding
and the plural rules are read and applied in Tondo. A number's digits come
from its `Display`, so an integer, a float and the decimal types to come
are written alike, and a float is rounded as people read it, from its
shortest digits: `2.675` is `2.68`.

`format.apply` takes a format chosen while the program runs, the same that
an interpolation takes after its colon, and, as there, the value's type
decides which options it may have: the compiler calls the version for
integers or floats, so a generic `T: Display` is all it asks.

`env.build()` is read when the program compiles, as `embed` reads files:
the name and the new `version` of `tondo.toml`, the git commit and whether
tracked files changed, the dependencies, and, from the runtime, the target.
It has no time of the build, which would make two builds of the same code
differ.

`trim` moved from the compiler's intrinsics to the prelude, to take the
characters to remove; Go's `Cut` is `splitOnce`, which gives an optional
pair rather than a flag.

## 2026-10-30 — Cryptography that does the safe thing by itself

The ninth lot. Each operation of `std.crypto` makes what is easy to get
wrong: `seal` draws its nonce and puts it before the data, so none is used
twice; `hashPassword` picks Argon2id and OWASP's parameters; a key has no
`Display`, so none is written to a log. `==` on `Array[Byte]`, and so on
digests, now takes a time that depends only on the lengths, which costs
nothing measurable and makes comparing an HMAC with `==` safe rather than a
mistake to remember; strings keep their usual comparison.

The primitives are ring's, which rustls already brings; PEM, DER, PKCS #8
and X.509 are read and written in Tondo. `verifyPassword` reads the hashes
of bcrypt, PBKDF2 (PHC, passlib, Django) and Argon2, so that a program
ported from another keeps its users. `PrivateKey.generate` takes a curve,
and RSA keys are only read, since ring does not make them.

A `Tls` is built from files or from PEM text, explicitly, rather than by
guessing which a string is; `trust`, a single argument of many functions,
takes either. `startTls` lets a connection that started plain go on over
TLS on a duplicate of its socket, which PostgreSQL and MySQL need.

## 2026-10-30 — Rules on the type, checked wherever a value comes in

The tenth lot. Rules are annotations on the fields of a type, as with Gin's
binding tags, but they belong to the type, not to one way of reading it:
the `Decode` the compiler writes checks them, so a value read from JSON,
YAML, a form, the command line or an MCP tool's arguments never breaks
them, and a program cannot forget a call. What is required is the type,
`T` or `T?`, and a closed set of values is an enum, so there is no
`required` or `oneOf`. The compiler checks each rule against its field,
which a tag in a string cannot have. A failure lists every violation: an
API answers `422` with all of them, a model corrects its call at once.
`Range` is a type of the prelude, so the bounds are `Min` and `Max`; an
integer literal is accepted for a `Float` field of an annotation, so that
`@validate.Min(18)` reads as written.

`App.client()` serves the app on a Unix socket of its own instead of
building requests in memory, so a test exercises the same server, streams
and all, without a port. `http.proxy` sends through a `Client`, whose hooks
observe what it sends. A closure is accepted where an optional function is
expected, which `sync.retry(work, when: ...)` needed.

## 2026-10-30 — Integers of any size and exact decimals

The first half of the eleventh lot. `BigInt` and `Decimal` are written in
Tondo over limbs of 32 bits held in `UInt`s, with Knuth's algorithm D, so
they gain from every optimization of the compiler and depend on no crate.
A `Decimal` keeps no zeros to spare, because `==` is structural: `1.50` and
`1.5` are one value, and a format fixes the decimals when it is written.
`/` of a `Decimal` gives 34 significant digits, as decimal128 does, and
`divide` what is asked.

A literal where one is expected is exact: the compiler hands its text to
the type, never through a `Float`, and a decimal literal wider than
`UInt128` is accepted there alone. Comparison operators stay closed to the
program's types, as 18.10 says; `Ordered.compare` orders these.

## 2026-10-30 — One type for a tree of files

The second half of the eleventh lot. A directory of the disk, the files
kept in the executable, files in memory, an archive's and layers of them
are one type, `fs.FileSystem`, with the operations of `std.fs` by the same
names, so what serves, loads or packs files takes any of them: a site is
served from the disk while it is written and from the executable when it
ships, a theme lies under a site's own files, and a test builds its tree in
memory. `embed.files` gives one, which still is a constant; `App.embedded`,
`Set.addGlob` and `Set.addEmbedded` went, as `App.files` and `Set.addFiles`
take a tree. Without dynamic dispatch, the tree is an enum of its kinds.

`std.archive` reads and writes zip and tar in Tondo, the runtime only
deflating, inflating and checking CRC-32. One API covers both, the format
read from the first bytes or the name; extracting refuses an entry or a
link that would leave its directory, as an archive made to attack does.


## 2026-10-30 — Databases whose queries cannot be injected

The twelfth lot. A query is a string literal where a `sql.Query` is
expected, so the compiler, which sees each interpolation, makes every value
a parameter and every `Query` a splice: the most natural way to write SQL is
the safe one, and SQL made at run time has to say so with `sql.raw`. A
`String` is not a query, and the error says how to keep a part of one. Rows
read through `std.serialization`, so a record, a scalar, an option, a JSON
column or an array reads as any format reads it, rules of `std.validate`
included.

`use` wraps each statement as `App.use` wraps a request, with what
OpenTelemetry names (`system`, the text, the database, the host), so a
tracer is a middleware and no driver knows about traces. A transaction
hands `work` a `Database` pinned to its connection, so code that takes a
`Database` works inside one without a second type, and a panic rolls back
before it goes on.

SQLite is the C library in the runtime, which a program without `std.sql`
does not link. PostgreSQL and MySQL are written in Tondo over `std.net`,
SCRAM, MD5, `caching_sha2_password` and RSA-OAEP included, so they gain
from the compiler and need no crate; a connection prepares a statement once
and keeps it, and asks PostgreSQL for the types of its parameters, as pgx
does, rather than guessing them. TLS always verifies the server, unlike
libpq's `prefer`; instead, `prefer` skips TLS for a server on the same
machine, which no network reaches, and a certificate that does not verify
says to give its authority or to disable TLS. Taking over a socket for
`startTls` no longer ends its writing when the plain stream is freed.

## 2026-10-30 — Comparison operators by `Ordered`

A type that implements `Ordered`, and a `T: Ordered`, gets `<`, `<=`, `>`
and `>=`, which are `Ordered.compare(a, b)` against zero: a price is
`price < limit`, not `Ordered.compare(price, limit) < 0`, and generic code
reads as concrete code does. Arithmetic stays with `Additive` and
`Number`, and equality stays structural; `Bool` keeps no order operators
although it implements `Ordered`. A type without it gets an error that
writes the `impl`, and a failed `assert` shows both values, not the result
of `compare`.

A type that takes a name of the prelude is still an error, since every
module sees those names, but the rest of the program is checked against
it, so the error is the only one instead of one at each use and in each
implementation derived for it.

## 2026-10-30 — Images, as a site makes them

The thirteenth lot. Decoding and encoding PNG, JPEG, GIF and WebP is the
`image` crate's in the runtime, as a codec is the kind of large crate
that belongs there, and only a program that imports `std.image` links it;
what a site does with them, fitting, filling with an anchor, keeping
proportions, is Tondo. An image never changes, so its copies share the
pixels and each operation gives a new one, which the runtime makes on the
host's threads. A photo comes up the way its EXIF orientation says, and
`info` gives that size from the header alone, as a page's markup needs.

## 2026-10-30 — Certificates that a server obtains by itself

The fourteenth lot. `acme.tls(["example.com"], email: ...)` is all a
server writes to have Let's Encrypt's certificates, obtained and renewed
while it runs. The authority checks each name with TLS-ALPN-01, which the
server answers on its own port, so nothing besides it has to be reachable,
as port 80 for HTTP-01 would be. The protocol is Tondo over `std.http`
and `std.crypto`: JWS with ES256, and the certificate request and the
check's certificate written in DER, so the runtime only had to learn to
show certificates that change.

That is `net.Certificates`: a server that shows them takes the last one
set at each handshake, and a client that asks for a protocol it keeps a
certificate for, as `acme-tls/1`, gets that one. A `Tls` names its
certificates to the runtime in its `certificate`, so `Tls` and the
natives of every listener stay as they were. ACME lives in a module of
its own, `std.acme`, because `std.net` cannot use `std.http`, which is
built on it.

## 2026-10-30 — GraphQL from the program's types

The fifteenth lot. As `std.mcp` serves a value's methods as tools,
`std.graphql` serves them as fields: the schema is the types the program
already has, their fields, their `pub` methods with parameters as
arguments, their docs as descriptions, so there is no schema language to
keep in step with code. Queries are parsed and run in Tondo over
reflection; introspection describes the schema to the tools of GraphQL,
and GraphiQL is a `GET` away. Server first: a typed client, subscriptions
and batching of fields come when a program needs them.

A root holds its state, as a database or a `std.sync` value, which
reflection now carries without looking inside, so such a record can be a
root of `std.graphql` or `std.mcp`; the schema leaves those fields out.

## 2026-10-30 — Servers that hold up in production

The sixteenth lot, from the second review of Gin, Hugo, dd-trace-go and
ddtest: what a server open to the internet needs and the first one lacked.

The limits are on by default, since a server that has to be told to
protect itself usually is not: 10 MiB of body, 64 KiB of head and 30
seconds to send it, which also closes a kept-alive connection that sends
nothing and a TLS handshake that never ends. `Limits` changes them for a
server and `Request.limitBody` for a route, as one of uploads needs. The
body is limited where it is read, so the handler's `?` answers `413` with
the way to raise it.

A client that goes away cancels the task that answers it, with the
cancellation tasks already have: the server drops the response nobody
waits for, and a guard in it cancels the task, which stops at its next
wait and runs its `defer`s. In Go a handler has to watch its context to
stop; here every wait is a place to stop.

`queryAs` and `formAs` read with the `Decode` the compiler writes for
JSON, through a `Source` that matches names as clients write them,
`page_size` or `page-size` for `pageSize`, and makes a list of a single
value when the field is one. No field needs a tag, as Gin's binding does.

`compress` is a middleware, so it compresses what the ones after it
return; zstd wins a tie because it is faster for the same size. A
WebSocket's handler gets its request and runs inside its route, so the
contexts the middleware sets, as the user or the trace, reach it.

## 2026-10-30 — What the `defer`s of a cancelled task still do

Since a client that leaves cancels its handler, cancellation is common in
servers, and it showed a gap: in a cancelled task every operation that may
wait ended at once, even one that would not have waited. A `defer` that
took one from a counter under a `Mutex`, or sent a last value to a channel
with room, did nothing, and said nothing. A `defer` is there to leave the
state right, so in the `defer`s of a cancelled task, and in what they
call, what needs no wait is done; a wait they need ends at once, and ends
that `defer`, as before.

Outside the `defer`s nothing changes: a cancelled task stops at its next
wait without doing it, so a worker cancelled between two jobs does not
take one from a queue only to drop it.

The runtime counts the `defer`s a task runs. In them, the check after a
wait unwinds only when that wait ended at once, so the functions a `defer`
calls go on after the lock they took.

## 2026-10-30 — Values the whole program shares, in contexts

Porting ddtest showed 116 regular expressions compiled once at the level of
a Go package. Tondo had no place for them: a `const` is evaluated when the
program compiles and cannot hold a compiled pattern, a pool or a `std.sync`
value, and a context field had to be `T? = none`, set with `with` in
`main`, so every read handled the `none` and a library could not own its
patterns.

Rather than a new kind of global, a context field's initial value may now
be computed: once, the first time the program reads it, and from then on
every task shares it. It is lazy, so importing still runs nothing and the
order of files never matters; it is a context, so a test replaces it with
`with` for a block. A `?` there panics at the read that computed it, and
the next read tries again; a literal pattern of `Regex.compile` is checked
when the program compiles, so that `?` does not fail. A `with` copies the
other fields without computing them: the table of contexts knows which
fields a `with` set, and a computed one it did not set is read from where it
was computed.

`static` and a top-level `let` were the alternatives. A `let` at the top of
a script is already a local of its `main`, so it would mean two things, and
`static` would be one more concept beside contexts.

## 2026-10-31 — Files of each system

Code that only exists on one system, as the C a library calls through
`inotify` on Linux and FSEvents on macOS, goes in files that their names
mark: `watch_linux.to`, `watch_macos.to`, and `signals_unix.to` for both,
the way `_test.to` marks tests. The specification had kept conditionals
out of the code and promised source sets declared in the manifest; a
suffix keeps the first promise without the configuration, and is the
convention a reader, an agent and Go already know.

A program built for one system must still be right for the others, so
`tondo check` and the language server check every system the package runs
on and name the system of an error that only one has. What common code
calls must exist for each with one signature: a name missing for a system
says where the others declare it and what to write, and two heads that
differ are an error on both. `systems` in `[package]` is for a package that
only runs on some.

In the editor those files are checked code, so they are never greyed out:
a badge says their system, and a file of the other system is read as that
system reads it. What does not run on this machine says so where it would:
its breakpoints stay unverified with the reason, its tests show without a
run button, and coverage names its files apart instead of counting them as
code that never ran.

## 2026-10-31 — Process groups, asked for

The seventeenth lot starts with what ddtest asks of the system. It runs
`go test`, which builds and runs a program for each package: cancelling
`go test` alone leaves those running. `Command.group()` starts a command
in a process group of its own, and `cancel` and `signal` then reach the
group whole. Go asks for `Setpgid` and a negative pid, Rust for
`process_group(0)` and libc; .NET's `Kill(entireProcessTree: true)` walks
the tree instead, and misses what is born while it walks or was already
orphaned. A group misses nothing, and on Windows the same `group()` will be
a job object.

It is asked for, not the default, because the terminal's Ctrl-C reaches
the group in the foreground: a command in its own group no longer gets it,
and the program decides, with `process.signals`. That is what a tool that
runs tests wants, and not what an interactive program started by one does.

`env.user()` and `env.system()` cannot change while a program runs, so a
private context of `std.env` keeps them, computed the first time they are
read: the standard library's own use of initial values computed once.

## 2026-10-31 — Config files edited in place, and tools that wrap tools

The rest of the seventeenth lot is what tools of the ddtest kind write and
read. A tool that changes a CI file or a project's settings has to leave
what a person wrote: re-encoding the YAML loses every comment and the order
of keys. `yaml.Document` keeps the text and where each node is in it, and
`set` replaces only the characters of the value it changes, as an editor
would. It is a document of a config file, in memory whole; what is too big
for that wants an event reader, which can come later. `writeTo` is there so
that writing it back does not make a second copy, which `text()` would.

A merge key `<<` is how such files share settings, and without it `job.x`
was `none` where the reader sees a value; it is resolved where every map
is made, so `parse`, `decode` and `Document.value` agree.

`@omitNone` exists because APIs differ on whether a missing value is
`null` or no key; `null` stays the default, as `encoding/json` and serde
write, and the annotation goes on the field or on the whole type, where a
request of a dozen optional fields wants it. `htmlSafe` is asked for, as
Go escapes always, because only JSON put inside HTML needs it and the
escapes make every other JSON harder to read.

In `std.cli`, a command's options also work after its subcommands, because
`app run --verbose` is what people type. `@cli.Rest` is for a command that
runs another, as `ddtest run -- go test -v`: it gets what the command does
not read, as written. `env` became a list because a renamed variable has to
keep its old name working for a while, as `DATADOG_API_KEY` beside
`DD_API_KEY`; the first that is set wins, and the help names them all.

## 2026-10-31 — Templates that build sites

Lot eighteen starts with what Hugo's templates do that ours could not.
A partial takes several values in Hugo through `dict`, a map of any
values; Tondo's maps hold one type, and a loaded template that allowed
mixed maps would stop compiling when moved into the program. `include`
takes values by name instead, as a call does: compiled, they are a record
of their own, typed; loaded, a map the included template reads by name.
Its literals follow the language's rules in both modes, and a loaded
template that writes `Card { ... }` is told to name the values or build
the record in a function, since it cannot know the program's types.

A computed name is a string with values in it, as everywhere in Tondo,
which has no `+` for strings: `{{ include "summary/{kind}.html" self }}`.
A compiled template cannot compute it, since it reads what it includes
when it compiles.

The functions of a `Set` are called as `reflect.call` calls them, and that
call now takes what a call in code takes: defaults, variadic arguments,
and any value for a parameter `reflect.Value`. A function that fails is
the template's error, as the spec already said.

The values of JSON, YAML and TOML read as what they hold rather than as
enums with methods: `{{ data.count + 1 }}` instead of reaching into
`Number`, because data files are what a site's templates read most, and
Hugo reads them that way.

Giving those values `Reflect` meant importing `std.reflect` into the
formats, and a program that brings `std.reflect` gets the trait derived
for all its types, which every program that read JSON would then pay for
in compile time. A module that imports it only for its own types no
longer brings it; `std.uuid`, `std.math` and the parts of `std.time` and
`std.sync` had done so already without need.

## 2026-10-31 — Dates that move along the calendar

A month after January 31 is the last day of February, as .NET, Java and
jiff answer, rather than March 3, as Go's `AddDate` normalizes it: a
monthly date that drifts into the next month surprises everyone who meets
it. `addDays` in a zone keeps the clock's time, so the day the clocks
change is 23 or 25 hours long, while `add(time.hours(24))` is exact time;
both are wanted, and the names say which is which. The week of ISO 8601 is
a value of its own, `IsoWeek`, because its year is not the date's near
January 1, and a week and its day read a date back with `%G-W%V-%u`.

## 2026-10-31 — Unicode's categories and each language's order

The categories and scripts come from ICU4X's `icu_properties`, which the
runtime already linked through URLs, so they cost 93 KB to a program that
asks for them and nothing to one that does not. `Script` is an enum of the
178 scripts that have characters, so a misspelt one is a compile error
rather than a string that never matches.

Collation is ICU4X's too: the Unicode Collation Algorithm with CLDR's
orders, which is what Hugo's `collate` and every browser use. Its data for
every language adds 1.36 MB to a program that makes a collator, the same
for one language as for all, since the language arrives while it runs; a
program without one carries none of it. Cutting the languages would need
data generated for Tondo and would save about one megabyte, only to
programs that sort text for people, so all of them stay.

## 2026-10-31 — Image filters in Hugo's scales

Hugo's `images.Filter` takes brightness and contrast from -100 to 100,
saturation to 500 and hue in degrees, through the gift library; `adjust`
takes the same numbers with the same formulas, so a theme's filters port
by copying them. They are one call with named values rather than one
method each, because a site adjusts several at once and the order they
apply in is then fixed and written down. `colors` answers Hugo's `.Colors`
with the most frequent exact color of each group of similar ones, so a flat
red is `#ff0000` and not an average its edges blurred.

## 2026-10-31 — Profiles of each period

A continuous profiler sends a profile of each minute: Datadog's does, as
Go's delta profiles. The totals that grow from when sampling began, what
was allocated and waited, subtract: `later.since(earlier)` gives the
minute, by stack and labels, and keeps what is alive now as it is.
The processor time does not keep totals: labels by span would make them
grow without end in a process that runs for days, so `cpu.period()` hands
over what was sampled and goes on. Each profile says how often it samples,
`every`, with today's rates as the defaults, since Datadog samples the
processor each 10 ms and a busy server may want fewer samples of memory.

## 2026-10-31 — A body read once, given on by hand

The third review found a middleware that read a request's body leaving
the handler an empty one, with no error. Keeping every body read in memory
for whoever reads it next would hold whole uploads and streams; instead a
body is still read once, reading it again is a panic that says how to give
it on, and `request.withBody(bytes)` gives a middleware's bytes to the
handler, which reads them as text, JSON or parts, as Gin's middleware puts
the body back. A route added twice or a parameter that is not a whole
segment were silently accepted and never answered: now they are errors, at
compile time for a literal path.

## 2026-10-31 — Defaults for a record's fields

Gin's and Hugo's configurations, and dd-trace-go's options, are records of
many fields of which a program sets a few: without defaults, Tondo had a
constructor for each, or every field written in each literal and an
`Option` for each one a file may leave out. A field now declares its
default, a constant as a parameter's, which a literal that leaves it out
takes and decoding too when the input lacks it, so one record serves the
code and the file. Being constants, they run no code where nothing is
written, as the specification's records promise. A record whose `priv`
fields all have defaults is built outside its module with the rest, which
keeps state a module owns, such as a URL's raw path, out of the hands of
those who build it.

## 2026-10-31 — Unions as the member they hold

Gin's handlers answer `gin.H`, a map of mixed values, and Hugo's and
ddtest's files hold values that are a string in one place and a list in
another: Tondo had to declare an enum, with a tag no format writes, or
reach for `json.Value`. A union is now written as the member it holds, as
every one of those formats does, and read as the first member that reads
the value. The language says `A | B` is `B | A`, so the order cannot be the
one written: members that read fewer values go first, floats after
integers, `String` after what reads text, as dates and enums, and a
format's `Value` last, which settles `Int | Float` and `Status | String` as
a reader expects. A tuple is a list, as JSON and YAML write pairs. The
compiler writes each union's code where its members are known, since a
union is no type an implementation in Tondo could name.

## 2026-10-31 — Failure: what the program was doing

dd-trace-go and ddtest wrap errors with what they were doing, as Go's
`fmt.Errorf("...: %w", err)`, and look for a cause by its type, as
`errors.As`. Tondo's typed errors are right at an API's border, where a
caller matches each case; inside a program they made every function
declare a union of what its calls fail with, only to print it. `Failure`,
in the prelude, is a message with the error that caused it:
`.context("reading {path}")?` on a result or an option, and
`failure.find[E]()` through the chain. It is written in Tondo, with the
cause kept as the runtime keeps any value for reflection, so any error is a
cause, the standard library's too, which implement no reflection. A `?`
that would propagate another error into a `Failure` is an error that
proposes `.context`, rather than a silent conversion.

## 2026-10-31 — Contexts captured and given back

dd-trace-go hands a span to the code that runs next by returning a context,
and its queues carry the context of each message to the worker that
handles it. Tondo's contexts follow tasks and blocks, which covers a request
but neither of those: a worker handles each message in its own task, and a
function's `with` ends with it. `Contexts.current()` captures every context
as a value, and `with captured` gives them back until the block ends: the
queue keeps one with each message, and a function that starts a span
returns the contexts its caller activates. One value for all of them keeps
a library that adds a context working with queues that know nothing of it;
it shares the task's table, which never changes, so capturing copies
nothing.

## 2026-10-31 — Indexing text without a cursor

Hugo's code walks text by index, as Go's byte indexes do, and in Tondo each
`text[i]` counted characters from the start: 1.2 million of them took 38
seconds. A cursor type would have made such code rewrite every loop.
Instead the runtime remembers, for a few long strings at a time, whether
they are ASCII, how many characters they have and the last character it
found with its byte: an ASCII string is indexed directly, and another walks
from the last position, so a loop takes a step each and the 38 seconds are
a fraction of one. The header of a string stays as it is, since the code
generator, the debugger and live builds read it; a string's entry goes when
it is freed or a builder takes its block, so no new string at the same
address meets an old position.

## 2026-10-31 — A live build's new fields start with their defaults

When a reload adds a field to a type whose values are alive, they got the
empty value of its type, and a field with none, as an enum, kept the frame
on the old code. A field's default says what a value that does not give it
has, which is what a value made before the field existed is: the reload now
gives it that, and the empty value only to a field without one. The
compiler hands the defaults it checked to the reload as constants, so
nothing runs while every task waits.

## 2026-10-31 — Defaults that the command line and tools see

`std.cli` took an option's default as text, `@cli.Option(default:
"8080")`, written again beside a field that could now hold it typed. A
field's default reaches reflection as `field.default()`, and `std.cli`
gives it to a missing option, after the variables of the environment and
the text of `@cli.Option`, and shows it in the help as the line would write
it; `std.mcp` no longer requires such a field of a model, and a reflected
value without the field gets it. The text form stays for values a type
cannot hold as a constant.

## 2026-10-31 — `@name` outside the formats too

`@name` names a field or a variant outside the program, and the formats of
`std.serialization` honoured it, but `std.mcp`, `std.graphql` and a
template's `json` read and wrote values through reflection, which only
knew the program's names: a type that a file reads with `user_id` showed
`userId` to a model or a GraphQL client. Reflection now gives
`externalName()` beside `name()`, and those libraries name fields and
variants with it, in their schemas, the values they read and the ones they
write, while the program goes on with its own names. GraphQL's habit of
upper-case enum values is then `@name("ADMIN")`.

## 2026-10-31 — Replies a middleware changes, and forms with files

Gin's middleware writes a response's status and body after the handler,
logs its size and renders errors with its own headers; Tondo's middleware
saw the status and the headers but could not change the first, nor read
the body, and a handler's problem reached it as an error it could not add
headers to. A reply now gives its body when it holds it and takes another
status or body, and `Reply.problem` is the reply the server gives for a
problem. `header` replaces a header, as Go's `Set` does, since two
`Content-Type`s were never meant; `addHeader` adds a value for the headers
that repeat. A multipart form reads into a type as a URL-encoded one does,
its files into `UploadedFile` fields in memory, as Gin binds them; the
streams of `uploads` stay for files that do not fit.

## 2026-10-31 — A type's own rules

Gin's validator writes rules between fields, on each element and of its
own; `std.validate` had five annotations, and every new kind of rule would
have been another. A type that implements `validate.Rules` says the rules
its fields cannot, in Tondo, with paths from the value; reading checks them
after the fields' rules and reports them together, and `check` too, since
reflection carries them. The annotations stay for the rules a field says by
itself, which the compiler checks fit its type.

## 2026-10-31 — Time between the bytes of a body and of a reply

A client that sends a head and then a body slower than it promised held a
handler for as long as it liked: a probe waited 24 seconds, and a server
has a handler per such client. A body now has `bodyTimeout` between its
bytes, after which it is a 408, and a client `replyTimeout` to take more of
its reply, after which its connection closes and its handler is cancelled
as for a client gone. Both count from the last byte rather than in total,
as Go's `ReadTimeout` does not, so a large upload over a slow link ends
while a stalled one does not hold the server.

## 2026-10-31 — Time to finish after SIGTERM

dd-trace-go flushes its last traces when the process stops, with an HTTP
request that waits, and in Tondo every wait in the `defer`s of a cancelled
task ended at once, so the request never left. A global grace period would
have given every task time it did not ask for. Instead a `within` entered
in such a `defer`, or in a function it calls, lets its waits go on until
its deadline: the library that needs time says how much, in its own
`stop()`, and a program only writes `defer tracer.stop()`. Its deadline
still bounds it, so a flush that hangs does not keep the process from
ending.

## 2026-10-31 — Stopping a child gently, and programs with many names

ddtest runs `go test` and must stop it as `docker stop` would, with
`SIGTERM` and time to write its reports, and `cancel` sent `SIGKILL` at
once. `grace` on a command gives it that time, for `cancel` and for the
cancellation of the task that waits for it, so the structured way of
stopping work stays the one way; the same time bounds the wait for output
a descendant keeps open, Go's `WaitDelay`, since both answer how long a
process may take to let go. ddtest's go shim also decides what it does by
the name it was run by and then becomes the real `go`: `env.invokedAs()`,
`invokedAs` and `exec` give it both, and `splitWords` and `quoted` read and
write the command lines it passes on, as a shell would, expanding nothing.

## 2026-10-31 — The system's number in `FsError.Host`

A tool that fails on a full disk, a loop of links or too many open files
had only `host` to say why, and Go's `syscall.Errno` and Rust's
`raw_os_error` give the number. `Host` keeps it, `Host(Int)`, and shows
it with the C library's text of it, so the message a user reads names the
cause; the named variants stay the ones a program decides on. The
number is the system's, which differs between Linux and macOS, so it is
for messages and for the rare program that compares it with one it knows.

## 2026-10-31 — Tags of their own in YAML

GitLab CI writes `!reference [.setup, script]`, CloudFormation `!Ref`, and
`std.yaml` failed on any tag outside the core schema, so neither file could
be read. `yaml.Value.Tagged(tag, value)` keeps the tag and writes it back,
which a tool that edits such a file needs, while a type decoded from it
reads the value, as it would without the tag, and the accessors of
`yaml.Value` see through it. The tag reaches `yaml.Value` through `tag()`
on `Source`, which other formats leave `none`, rather than as an event
every `Decode` would have to skip. A tagged scalar is text: the tag, not
the core schema, says what it means.

## 2026-10-31 — One path of keys and indices for every format

GitLab CI names jobs `.setup`, and `find` split its path at every dot, so
such a key could not be reached, nor could an item of a list but through
`yaml.Document`. Every `find`, of each format's `Value` and of
`yaml.Document`, now reads the same path, `jobs[".setup"].script[0]`, as
JSONPath and jq write a key in brackets, and `json.Value` gains the
`find` it lacked. The runtime parses it once for all, and writes the paths
of a document in the same form, so a path a user writes and the one the
outline gives compare as text.

## 2026-10-31 — What a YAML node is, and inserting and removing

A tool that rewrites CI files, as ddtest does with GitLab's, must know
whether a value is a literal block or a flow list to keep its look, and
must add a step in the middle of a list or drop a job, which `set` could
not. `Node` gains `kind`, `style`, `tag` and `anchor`, read from the text,
since the parser gives anchors only as numbers, and `Document` gains
`insert` and `remove`, which work on the lines of a block collection as
`set` does and write a flow one again. An item inserted goes before the
comment lines above the one it displaces, and a value removed takes those
above it, since `find` gives them as its comment.

## 2026-10-31 — Several JSON values in one text or stream

`go list -json`, `docker inspect` piped through jq and JSON Lines logs
write one value after another, and `std.json` rejected anything after the
first. The reader gains a mode that reads the next value where one ends,
with or without space between them, as Go's `json.Decoder` does, which
`parseAll`, `decodeAll` and `values` use; `decode` stays strict, since a
second value there is a mistake. `values` is an iterator like `csv.rows`:
a stream from a process gives each package as it is written, and an
error ends it.

## 2026-10-31 — MessagePack integers past `Int`

MessagePack holds unsigned integers up to 2⁶⁴ − 1, as Datadog's agent
protocol and Go's `uint64` fields write them, and `messagepack.Value` read
those past `Int.max` as a `Float`, which lost their low digits. They read
as `Value.UInt(UInt)` now, and are written back as the same integer; a
type with a `UInt` field already read them exactly.

## 2026-11-01 — Variables, `if` as a value and `define` in templates

Hugo's themes count and sum in loops with `$n := 0` and `$n = add $n 1`,
choose a word with `cond`, and keep partials in the file that uses them
with `define`, and a Tondo template could do none of it. `var` declares a
variable that `=` and `+=` change, inside loops too, and only a `var`
changes: the check is made when the template is read, from the blocks
around the assignment, so a loaded template fails when added and a
compiled one when compiled, rather than when written. An `if` with braces
gives a value, as in code, and needs its `else`. `define` writes a
template in the text of another; in a `Set` it joins the set by its name,
as Go's does, and a compiled template finds it in the templates being
written before it looks for a file. The syntax is the same in both kinds,
since the parser is shared.

## 2026-11-01 — Lossy WebP with libwebp

Hugo writes WebP with loss, at a quality, and `std.image` wrote it only
without, as the `image` crate does, so a site's photos came out several
times larger than Hugo's. Lossy WebP needs Google's encoder, and no Rust
one is close to it, so the runtime builds libwebp's C with `cc`, as it
builds SQLite, through `libwebp-sys`; only programs that import
`std.image` link it. `encode` and `save` take the `quality` of JPEG for WebP too,
85 unless given, and `quality: 100` keeps it without loss, so WebP is
lossy by default, as in Hugo.

## 2026-11-01 — A photo's EXIF

Hugo's galleries sort photos by `.Exif.Date` and put them on maps with
`.Exif.Lat` and `.Exif.Long`, and `std.image` read the EXIF only to turn
an image. `image.exif(bytes)` gives the date, the place, the camera, the
orientation and every tag as text, from `kamadak-exif`, a reader in pure
Rust, without decoding the pixels. EXIF dates are a camera's clock with
no zone unless the photo gives its offset, so `date` uses the offset when
there is one and reads the clock as UTC when not, which keeps photos in
the order they were taken without guessing the zone.

## 2026-11-01 — Walks that prune and skip

A tool that walks a project must not go into `node_modules` or `.git`,
and one that walks a system must go on past a directory it may not read,
and `fs.walk` did neither: it read everything and failed on the first
directory it could not. `enter` decides, for each directory, whether the
walk goes into it, as Go's `filepath.SkipDir` does, and lists it either
way; `skipUnreadable` leaves out a directory below the root that cannot
be read. Without `enter` the runtime still walks the whole tree at once;
with it, the walk lists one directory at a time, since `enter` runs in
the program. `fs.watch` now skips such a directory too, rather than
watching nothing.

## 2026-11-01 — A literal can be a `Byte`

A `Byte` literal had to be `Byte(255u8)`, so a file's magic number was
`[Byte(0x89u8), Byte(0x50u8), ...]` and a pixel `pixels.push(Byte(255u8))`,
and a program that wrote `255` where a byte goes got E1102. The rule kept
binary data apart from arithmetic, but a literal is a constant whose
value the compiler sees, and the type keeps that boundary: a `Byte` still
has no arithmetic, and a variable still converts with `Byte(n)`. So an
unsuffixed literal from 0 to 255 is a `Byte` where one is expected, in
expressions and in patterns, and one out of range is a compile error.

## 2026-11-01 — `contains` on arrays

`Map`, `Set` and `String` had `contains`, and `Array` and `Deque` only
`value in array`, so `names.contains("ada")`, which a program from any
other language writes, was E1001. `Array.contains` and `Deque.contains`
ask what `in` asks, for an `Equatable` element, and a `Set` template has
the array's too.

## 2026-11-01 — Protobuf as a format with a trait of its own

gRPC needs protobuf, and the earlier design generated codecs from `.proto`
files only. Protobuf is now a format like XML, whose records carry their
field numbers as annotations, so a program can declare a message without
a `.proto`, and the `.proto` reader, next, writes such records. It does
not ride on `std.serialization`, though: that protocol's events say what a
value is as it goes, and protobuf's bytes do not, since a length-delimited
field may be text, bytes, a message or packed numbers. So the compiler
implements a trait of its own, `Message`, field by field with each field's
exact protobuf type, on a native writer and reader of the wire format; the
same records still get `Encode` and `Decode` for JSON and the rest. The
numbers come only from the annotations, never from field order. An enum
is open with an `Unrecognized(Int32)` variant, a name many protos do not
use themselves as they use `UNKNOWN`. A field with a default is skipped
while it has it, which is proto3's implicit presence and the approved
reading of proto2's `[default = v]`; a field with neither default nor `?`
is proto2's `required`.

## 2026-11-01 — A `.proto` file is a module

Services and libraries publish their `.proto` files, OTLP and Datadog's
among them, and a program should use them as they are, with no generator
to run or configure. A `.proto` file among the sources is a module of its
own, named by its file, which the compiler writes from it as the
annotated records and enums of `std.protobuf`; the files of a package are
read together, since one names another's types by their full names, and
an error is reported at its line of the `.proto`. A `.proto` file is a
module rather than part of its directory's, as `.to` files are, because
protobuf's own unit is the file and two files of one directory often hold
types of the same names. Enum values drop the enum's name, as protoc's
other languages do, and keep it as their `@name` for JSON; generated
enums show that name. The well-known types are Tondo's own types, so a
`Timestamp` is a `time.DateTime` everywhere.

## 2026-11-01 — gRPC from the services of `.proto` files

A `service` gives a trait of its calls, a function that makes a server's
`grpc.Service` of a value that implements it, and a client, so that
`app.grpc(users.userService(Users { db }))` is checked by the compiler
like any other implementation: a call with the wrong types, or one
missing, is an error at the value, not a failure when a client calls.
The service is routes of the `http.App`, one `POST` a call, so gRPC shares
a port and its middleware with the rest of a server, over HTTP/2 with or
without TLS. A method runs in a task of its own, whose messages go out as
it sends them through a pipe; the headers wait for its first message, so
that `setHeader` works until then, and the status goes in trailers, which
`std.http` sends after a streamed reply with `Reply.stream(trailers:)`.
What a call carries besides its messages comes by context, `grpc.call()`,
as a deadline does, instead of a parameter every method would take. A
stream from the server is the prelude's `Sender`, so a method sends as a
producer task does; when the client goes, sending fails and the method's
task is cancelled. A `within` around a call becomes `grpc-timeout`,
rounded up so the server's deadline is never the earlier one, and the
client gets its `within`'s `TimedOut` as any other wait does. The client
keeps one HTTP/2 connection a channel, shared by its calls and made again
when it drops; the value behind a service is copied into each method, as
closures copy what they capture, so it holds handles, not a `Sender`.

## 2026-11-01 — gRPC's interceptors

A tracer, an authenticator or a log wraps every call of a service or a
client, which `onCall` does with `next`, as `App.use` and `Database.use`
do, the first added outermost. On a server, `next` runs the method in the
call's own task, so a context the interceptor gives with `with` reaches the
method and what it calls, which is how a span would. On a client, a unary
call is wrapped where it is made; a stream returns as soon as it starts and
is read afterwards, so its interceptors run in a task of their own, with
the caller's contexts, and `next` gives the status when the call ends,
while the caller reads the call that `next` made: one `onCall` serves the
four kinds of call, instead of a second pair of hooks for streams.
`onMessage` shows each message as a `reflect.Value`, which reads any
message without knowing its type, so messages implement `Reflect`.

## 2026-11-01 — gRPC's health and reflection

Load balancers and orchestrators ask a server's health through
`grpc.health.v1`, and tools as grpcurl call a server they have no `.proto`
for through reflection, which gives the descriptors of the files that
declare its services. The compiler writes each file's `FileDescriptorProto`
itself, as protoc does, byte for byte for a file without options, into a
`fileDescriptor()` of its module, so a program needs neither protoc nor a
generated file; a service of the file carries it, and
`grpc.reflection([services])` gives those it is handed, rather than every
service of the app, since an app does not keep its routes' kinds. The
messages of both protocols are written in `std.grpc` by hand, as
`google.rpc.Status` is, since the standard library is not derived; the
descriptor of `health.proto` is a constant, which protoc and the compiler
both write the same.

## 2026-11-01 — A line that starts with `-` begins a value

A line that started with a binary operator continued the one before, `-`
among them, so a `match` could not have a negative arm after its first,
`-1 =>`, and the arms protobuf derives for an enum with a negative value
did not compile. A `-` at the start of a line now begins a negative value,
as `+`, `*` and the others still continue the line; a subtraction that goes
on to the next line ends the first with `-`, as `tondo fmt` writes it. Code
that split a subtraction before its `-` now has a statement of its own,
which warns (`W1014`), so it does not change its meaning in silence.

## 2026-11-01 — A gRPC method is cancelled when its client goes

A method used to learn that its client was gone only when it sent a
message, which failed; one that waited, as a health watch with no change or
a stream of rare events, kept its task and what it held until it sent, which
might be never. Now the method's task is cancelled as soon as the client
goes, before the reply or during it, as a handler of `std.http` is: the
server lets go of the reply, and so of the pipe the method writes to, and a
pipe whose reader goes before its writer cancels the task that follows it.
A streamed reply now reads its pipe without a thread, so that letting go of
the reply lets go of the pipe at once rather than after the next write.

## 2026-11-01 — Descriptors with the options of descriptor.proto

A real `.proto` file has `go_package`, `java_package` or `deprecated`
options, reserved numbers and `extend` blocks, which protoc writes into its
descriptor, so the compiler's descriptor of such a file differed from
protoc's. The parser now keeps them and the descriptor writes them in
protoc's order, the options of `descriptor.proto` by their numbers. An
option of the program's own, `(name)`, is still left out: writing it needs
the extension that declares it and its values in protobuf's text format,
and tools that read a server's reflection do not need it.

## 2026-11-01 — gRPC compresses with gzip when asked

Every client and server reads messages compressed with gzip and says so in
`grpc-accept-encoding`, but sends them compressed only when the program
asks, a channel with `compression: Compression.Gzip` and a service with
`compress()`, as grpc-go does: compressing costs time on every message and
gains only on large ones, which the program knows and the library does not.
A server answers compressed only a client that reads gzip. A message that
would grow past 16 MiB once decompressed is refused, so that a small one
cannot take all the memory.

## 2026-11-01 — proto3's JSON, and the program's table of messages

gRPC's JSON, as Connect and transcoding speak it, and the JSON other
languages write of a message, follow proto3's own mapping, not std.json's:
lowerCamel names, 64-bit integers as text, enums by name, the types of
`google.protobuf` in their own forms. The compiler writes each message's
JSON beside its bytes, and keeps a field's `.proto` name in
`@protobuf.Field(name:)` for `protoNames`. An `Any` in JSON holds its
message's fields, so writing one needs the message's type from its name:
the compiler lists every message type of the program for `std.protobuf`,
as a table it reads at run time, so that an `Any` of any message the
program has needs no registration; one of a type the program does not have
fails, since nothing could say what its fields are.

## 2026-11-01 — CORS as a middleware of App

A page served from another origin, as a gRPC-Web or Connect client in a
browser, can call an app only when the app answers the browser's preflight
and says in each reply that the page may read it. That is the same for
every API an app serves, so it is `App.cors`, a middleware of the app
rather than of gRPC. It lets in only the origins it is given, or `*`;
with cookies (`credentials`) it names the origin, since browsers refuse
`*` then. It sends `Vary: Origin` on every reply, without an `Origin`
too, so that a cache does not give one page's reply to another, and it
adds its headers to a `Problem` as well, so that a page reads the error
instead of a CORS failure.

## 2026-11-01 — gRPC-Web on the same routes

A browser cannot speak gRPC: it gives a page neither HTTP/2 trailers nor
always HTTP/2. gRPC-Web carries the same frames with the trailers as a
last frame of the body, and other languages need a proxy, as Envoy, to
translate it. A Tondo server reads the request's content type and answers
gRPC-Web itself, on the same routes and with the same methods, so a page
calls a service with no proxy and no other declaration; the text form,
Base64 for clients that cannot read bytes as they arrive, decodes each
padded piece a client sends. Only the server speaks it: a Tondo client has
HTTP/2 and speaks gRPC.

## 2026-11-01 — Connect on the same routes

Connect is a protocol of the same calls that plain HTTP clients can speak:
a unary call is a `POST` of the message alone, in protobuf or proto3's
JSON, and its reply is the message or a JSON error with the HTTP status of
its code, so `curl` and a browser's `fetch` call a service without a
library. Its streams carry gRPC's frames and end with a JSON message
instead of trailers. A Tondo server answers it on the same routes as gRPC
and gRPC-Web, by the request's content type, with the same methods; a
method marked `NO_SIDE_EFFECTS` also takes a `GET`, which caches and CDNs
can keep. Reading JSON ignores fields the message lacks, as Connect's own
servers do, so a newer client can call an older server. Only the server
speaks it, as with gRPC-Web: a Tondo client has HTTP/2 and speaks gRPC.

## 2026-11-01 — Client balancing, and targets with several servers

A service of several servers is reached through a name with several
addresses, as Kubernetes' headless services give, or through a list of
them. A channel now keeps a connection per address and spreads its calls
as gRPC's two common policies do: `PickFirst`, the default as in every
gRPC library, keeps them on the first address that connects, and
`RoundRobin` gives each to the next of those that answer. A target may
list servers separated by commas, which needs no DNS and lets a test run
several servers on one machine. An address that fails waits as gRPC's
connection backoff says, so that a server that is down is not hammered,
and a call while every address waits fails at once with `Unavailable`
rather than wait, as gRPC does without wait-for-ready. The channel
resolves its hosts again when a connection drops or an attempt fails;
like gRPC's DNS resolver it does not poll, so servers added behind a name
are found when a connection changes.

## 2026-11-01 — A server that stops drains its connections

`Server.stop` stopped taking connections and returned while the ones
clients kept open were still taking requests, which it then answered
`503`: the first call after a server stopped failed in 8 of 20 tries, and
in a deploy that restarts servers one by one every restart would fail
some calls. `stop` now drains, as gRPC's `GracefulStop` and Go's
`Shutdown` do: each connection is told to take no more, with a GOAWAY on
HTTP/2, answers what it has and closes, and `stop` returns once they all
closed. A reply that never ends, as a stream a client still watches,
would hold it for ever, so `grace`, 30 seconds by default as Kubernetes
gives a pod, bounds the wait; then what is left closes as it is.

## 2026-11-01 — gRPC's transparent retry

A server that goes away, as one draining when it stops, refuses the
streams it will not begin, and a connection that closes may drop a
request before sending it. Neither reached a method, so making the call
again cannot do it twice; gRPC's clients do it by themselves, once, and
a Tondo channel now does too, up to three tries, on the address its
balancing picks next. A call of one message keeps its bytes to send them
again; a call that streams its messages goes again only when it was never
sent, since the ones it sent are gone. A call a server began is not
retried: whether to do it twice is the program's to decide.

## 2026-11-01 — Round robin resolves its name every 30 seconds

gRPC's DNS resolver looks a name up again only when a connection drops,
so a round robin client keeps the servers it first found until one
breaks, and servers added behind the name, as a service that scales up,
get no calls; gRPC's answer is for servers to close their connections
from time to time. A Tondo channel with `RoundRobin` instead looks its
hosts up again once 30 seconds passed since the last time, in the
background of the call that notices it, which then does not wait: new
addresses are connected as round robin connects the others, and those a
name lost are dropped once their calls end. `PickFirst` stays on one
server and has no use for it.

## 2026-11-01 — A server that listens drains on SIGTERM

A deploy ends a server with `SIGTERM`, which cancelled the program: every
task, the handlers answering requests too, so those requests failed, and
the program ended within seconds. `App.listen` now receives `Interrupt`
and `Terminate` itself, through `process.signals`: the first drains the
server as `Server.stop` does, within `grace`, while its handlers go on,
and then ends the program as the signal would have, with its `defer`s
run and 130 or 143, so a program whose server listens in a task of its
own still ends. Its channel goes after the first, so a second signal
ends the program at once, as before. A program that wants to decide by
itself starts the server with `App.start` and listens for the signals.

## 2026-11-02 — The guide names every command and tool

The guide taught a dozen of `tondo`'s commands and almost none of
`tondo mcp`'s tools, so an agent learned the rest only by trying them.
`tondo guide cli` now goes through every command and `tondo guide mcp`
every tool, grouped by what they are for, and a test fails when the
table of commands or of tools has one they do not name, so that they
keep up with the toolchain without anyone remembering to.

## 2026-11-02 — The debugger shows what a block can use

Stopped in a test, the debugger showed the test's own bindings and none
of its suite's, though the test uses them: a test is a closure, what it
captured lives in its environment and not in its frame, and the suite's
frame was left out of the table of functions so that a failure's stack
would end at the test. Now a closure's frame shows its captures as its
bindings, the suites are in the table and a failure's report stops at
the test by itself, and `locals` adds the bindings of the functions a
frame is written in whose frames are below it on its stack, as a
closure's maker or a test's suites, each with where it is from. What is
not on the stack, as the maker of a closure called later from elsewhere,
is not shown: its values are gone.

## 2026-11-02 — Examples in the library, and finding what to call

Every standard declaration was documented, but none showed how it is
used, nothing searched the library, and an editor showed a module as
its name alone. Each standard module now starts with an example, a whole
program, and the entry points that need one carry it in their `///`; a
test compiles every one, so they cannot rot as the guide's cannot.
`tondo api std.http.Client` or `tondo api String` shows one declaration
with its methods, `tondo api --search` finds declarations by their names
and documentation, the MCP tool `api` does both, and the editor's hover
of a module and its search of symbols reach the standard library too,
those the compiler implements included.

## 2026-11-02 — The runtime counts gRPC calls

gRPC already had its observability hooks, `onCall` and `onMessage` on a
service and on a channel, as HTTP has `App.use` and `Client.use`. What
it lacked against HTTP were the runtime's own counters, which `tondo
counters` shows with nothing written: a channel's calls went uncounted.
The runtime now counts, on each side, the calls, those that run and
those that failed: a status other than `Ok`, or on a server a call its
client left before it ended. Metrics with labels, as by method or
status, stay the interceptors' to write, as the guide shows.

## 2026-11-02 — More of the editor from the language server

A review of the language server against the protocol found what an
editor could not yet do: complete a call's parameters by name or a
record's fields, write a keyword's construction as a snippet, show the
type of a closure's parameter, fade unused code, say what a keyword
does, show the values of bindings beside their lines while debugging
(`inlineValue`, which the debugger's `evaluate` answers), give the
tokens of some lines, follow files that change on disk, as after a
checkout, and offer an arm for what a `match` misses, which the
compiler gives as a fix that requires a decision, since its body
panics until written. The tokens are also counted in one pass now: a
large file took time quadratic in its lines.

## 2026-11-02 — An example for every type of the library

A module's example showed where to start, but a type met later, as the
`Response` a call gives or the `Event` a stream yields, still had only
its declaration. Every public type of the standard library now carries
an example in its `///`, a whole program that uses it as a program
would, compiled with the rest; a test fails when a new type has none.
Writing them ran each one, and what they showed was fixed on the way:
a URL host that RFC 3986 does not allow, `http.Saved` without a
decoder, protobuf fields typed with an alias, reflection without `Send`
and `Share`, and a `derive` error that did not say what to write.

## 2026-11-02 — Leading agents to `tondo api`

Agents wrote Tondo by guessing names from other languages, and only the
guide's overview told them to look first. The MCP server's instructions,
the `api` tool, the overview and the guides of the library, the CLI and
MCP now say to look up what a program calls before calling it, and that
a type's view has its methods and an example that compiles. The
compiler's errors lead there too: a name a module does not declare gives
the near one it does, with its repair, or the `tondo api` of the module;
a method a type of the prelude or the library lacks gives the `tondo
api` of the type. `tondo api` takes `http.Client` as the compiler writes
it, and a search ends by saying how to see one result.

## 2026-11-02 — Refactors that keep the program checking

The language server gained the editing help an IDE is expected to give:
the method an `impl` lacks and the fields a literal lacks, written by
repairs of the compiler so that `tondo fix` and agents get them too;
what a `?` or a `fail` propagates and where to, on hover; counts of
references and implementations over each declaration; and extracting
an expression into a variable or lines into a function. A refactor is a
guess about the program's meaning, so each is offered only after the
server has checked the program with it applied and found no error it did
not have, and only where moving code cannot change when it runs: not
past an `and`, out of a closure or an arm, nor lines that leave their
block or change a `var` of their function. They come when asked for, not
with the light bulb, so that selecting text does not check the program
again each time.

## 2026-11-02 — More help from the editor, and where a binding is written

A second round of the language server: a test for a function, written
into the module's tests to fail until it says what it checks; templates
after a value's dot, as `.match` and `.for`; the exits of a function
under `fn`, `return`, `fail` and `?`; the type of each line of a chain
of calls; inlining a `let` read once in the next statement, checked as
the extractions are; what fits the type expected first; `var` bindings
marked `mutable`, which VS Code underlines; and the colours of
`image.Color`. Writing the test of the marks showed that the binding an
assignment writes was no reference, so a rename left `sum += value` as
it was; it is one now.

## 2026-11-02 — Every missing arm, `pub` where needed, extraction of methods

A `match` that misses several cases gets them all as arms in one repair,
found as the exhaustiveness check finds one: each case missing is added
and the check asked again, with a value of an open domain written `_` so
that it covers what it says. A name private to another module gets the
repair that declares it `pub` there, a decision because it widens what
the module offers. Extracting lines lends a `var` of the function with
`mut` and makes lines that use `self` a method of the same type, still
offered only when the program checks with them. Imports and embedded
paths are links.

## 2026-11-02 — Names from modules a file does not import

`Uuid.v4()` without `import std.uuid` said only `unknown name`, and a
model or a person had to know where `Uuid` lives. The compiler now says
which standard modules declare a name and repairs it with the module
and its import: safely when one module declares it, as a decision for
each when several do, as `encode`, those the file imports first. The
editor completes the names of modules the file does not import, the
standard library's and the project's, writing the module and adding the
import, and finds symbols by the letters of their words.

## 2026-11-02 — Enums show their variant; references reflect; literals for generators

An enum whose variants hold nothing now implements `Display` without one
written: it shows its variant's name, as `High` for `Level.High`. Twenty
enums of the library could not be interpolated, as `hash.Algorithm` or
`fs.FileKind`, nor could a program's, and writing a `match` only to show
a name was the commonest friction of writing examples. The compiler
writes the implementation into the enum's module, for the library's
modules too, and its own enums, as `fs.FileKind`, show their names in
the generated code; an `impl Display` the module writes wins, and the
library's errors still show their words, as `not found`.

`Ref[T]` implements `Reflect`, reading through to its value, so a type
with a reference can be used by templates, MCP tools and GraphQL, and
`ReferenceKind.Ref` is what its description says, which nothing produced
before. A reference cannot hold itself, so reading through ends; setting
a part fails, as its value never changes.

`meta.literal` writes a text as the Tondo string literal that reads back
as it, for generators, which wrote their own escaping and missed `{`.

## 2026-11-02 — Diagnostics point to the other place

The diagnostics format had `related` from the start, but the compiler
left it empty, and an editor could not lead from a duplicate to what it
repeats. It is filled now where another place explains the error: the
first declaration of a name, field or variant declared twice, the
binding another would shadow, the method of a trait an `impl` does not
match or repeats, the `impl` another overlaps, the first of a repeated
`derive`. The language server gives them as `relatedInformation`, and the
text shows each as a `note`. An unused binding gets the repair its
warning described, renaming it `_name`, as a decision rather than a safe
repair, as an unused binding may be a mistake.

## 2026-11-02 — Tests and layout for every editor

VS Code runs and debugs tests from its Testing view, through
`tondo/tests`; other editors had no way to run one. An editor that does
not ask for `tondo/tests` gets *Run test* over each test and suite,
which the server runs itself with `tondo test` and reports in a message,
with the panic that failed it; VS Code, which asks, gets no duplicate.
Writing `}` lays out the block it closes, in every editor that formats
on typing, which the VS Code extension turns on as it does on saving.


## 2026-11-02 — A debugger that evaluates, watches and stops at errors

A condition compared one path with one literal, and `print` read a path,
so `items[i]`, `total / count` or `a > 1 and b` could not be asked. The
debugger now evaluates expressions in conditions, logpoints, `print` and
watches: bindings and their parts, literals, arithmetic, comparisons,
`and`, `or`, `not`, and methods that only read, as `length()` or
`contains()`. It reads values as it shows them and never runs the
program's code, so looking changes nothing and works in a stopped task;
calling the program's functions is left out for that reason. A condition
is checked when it is set: a name that is no binding there is an error at
once, not a breakpoint that never stops.

`watch` stops on the line after the one that changed a value, in the
watching frame or a call it makes, reading it at the task's probes rather
than through the processor's watchpoints, which see memory but not a
value that is rebuilt elsewhere; it ends with its frame. `catch error`
stops where an error starts: a `fail` of the program's code, or a `?` on
what a function of the standard library failed with, so that an error
that travels through several `?` stops once, where it began. `break fn`
stops where functions of a name start. A value is read whole and opened
one level at a time, so an array of any size opens, in pages, where the
answers clipped it at a megabyte before.

## 2026-11-02 — The debugger shows every task, and steps through the program

A task that waited in a channel, a lock or a `sleep` reached no probe, so
the debugger did not list it, and a hang was the case where it mattered
most. While the program stands still, `tasks` lists the tasks that wait
or are ready to run, from the frame pointer each saved as it switched
out, as dumps read them, and their frames show their bindings. Steps go
over the code the program's files do not have, the standard library's
and the derived, unless a step starts there, as other debuggers' "just my
code"; `steps all` goes in. A function's return tells a debug build's
runtime its value, which it keeps only while that task steps, so the stop
after leaving a function says what it returned.

## 2026-11-02 — `set` writes into parts of values

`set` changed whole bindings of a few types. It now takes a target, a
binding or a part of one, and an expression. The compiler describes in
the debug table how each binding's type is laid out: a record's or a
tuple's fields by offset, a plain enum's tag, an array's item size. The
runtime walks that description, so it writes in place without code of
the program running. An array another value shares is copied first, as
the program's own write would, but only when its items hold nothing
counted, because the runtime has no retain glue for them; otherwise the
change is refused with what to change instead.

## 2026-11-02 — Watches of shared values follow the value

The debugger showed a `Ref` or a `std.sync` cell as its type's name, so
shared state, where races hide, could not be seen. It shows what they
hold, as a record whose `value` is what the program's `.value` reads.

A watch belongs to a frame, as a local's value lives there; a cell's
value does not, and any task may change it. A watch whose binding holds
a `Mutex`, `RwLock` or `Atomic` keeps the cell alive and reads it at
every task's probes, taking it to read without waiting, and skipping a
check while a task holds it to change it, so it never reads a value
halfway through a change. It stops the task that changed it, which is
the one that runs on after the change. Watches take a condition and a
count of hits, as breakpoints do, so that a value changing in a loop
stops only where it matters.

## 2026-11-02 — Debugging a program elsewhere goes through a relay

A program on another machine or in a container serves the tools on a
local socket there, and a debugger had no way to it. Rather than a port
the program would open, with its own authentication, `tondo debug --via
"ssh host" <name>` runs `tondo relay <name>` through that command, which
joins its standard input and output to the program's connection. Access
is the command's: ssh's keys, docker's permissions. The relay passes
each piece on at once, as `std::io::copy` held pieces back between a
socket and a pipe and a debugger waits for every answer.

## 2026-11-02 — Going back is a history of values

Going back in a program can mean running it backwards, which needs every
source of nondeterminism recorded, as rr does, or running a frame again,
which repeats its effects. He chose a history instead: `record` keeps,
at each line of the program's own code a task runs, its stack and the
bindings its code can use, as the debugger shows them, clipped, in a
ring of the last lines. `back`, `forward` and `reverse` move through it
while the program stands still, and `locals`, `print` and `stack` read
the moment shown; nothing runs backwards, and `continue` goes on from the
present. It is slower while it records, as every line renders its
bindings, and it keeps the innermost frame's bindings only.

## 2026-11-02 — `goto` moves a frame as a reload does

Set Next Statement moves a frame to another line of its function without
running the lines between. A live build already moves frames at
statements, to the new code of a reload, with their values copied by
the names the points' layouts give. `goto` is such a move inside the
same code: `tondo` reads the build's manifest, finds the points of the
two lines, and lets the copies decide: a binding the target line uses
that holds no value at the current one refuses the move, and says so.
The runtime keeps the move for the stopped frame only, once, and the
frame shows at the target line until it goes on; a move the frame cannot
make where it stands is forgotten at its next line.

## 2026-11-02 — A debugger's call is a live patch

The debugger's expressions read values and never run code. A call of the
program's own functions needs its code, typed and compiled, so `call` is
for live builds and uses what a reload does. `tondo` adds a function to
the stopped function's file, in memory as an editor's unsaved text: it
reads the bindings the expression names from their addresses, which the
runtime passes, and returns the value's `Display`. The function is a root
of that analysis, as nothing calls it, and the patch adds it to a slot.
The stopped task, waiting in the runtime, calls it on its own stack, and
its probes let it run to its end. Conditions, watches and logpoints never
call, so that looking at a program does not change it unless asked.

## 2026-11-02 — Every tool of a running program takes `--via`

The relay serves any connection to a program, not only a debugger's, so
`ps`, `counters`, `trace`, `profile`, `dump` and `reload` take `--via`
as `debug` does: the connection they open goes through `tondo relay` on
the other machine, and `ps` runs `tondo ps` there. In `tondo mcp`, their
tools take `via`, and `debug_start` takes `test`, so that a model debugs
one test alone as `tondo debug --test` does; `goto` answers as a step
does, so that a session waits for the stop it brings instead of timing
out, and moving through a history that has nowhere to go says so.

## 2026-11-02 — A stop at once, and the cause of a task

A model debugging over MCP asked `tasks`, `stack`, `locals` of each
frame and the history in as many calls, each a round trip. `snapshot`
answers all of it at once. A task's stack ended at its first function,
though what started it is half of why it is there: the runtime keeps
where each task was spawned, and its spawner's, for dumps, and `stack`
and the editor's call stack go on with them, as async stacks do in other
debuggers. `locals` marks what changed since it last showed a frame, as
an editor highlights it; `until` runs to a line; a breakpoint can be one
task's; and `back until <condition>` searches the history rather than
stepping through it.

## 2026-11-02 — What a task waits for, and a deadlock's cycle

A task that waits showed where, not for what: a lock, a channel, another
task. Each task now records what it is blocked on while it blocks in a
lock, a channel's receive or send, or a join; a lock's holders come from
the locks each task holds, which the runtime keeps for dumps. `tasks`
says what each waiting task waits for and who holds it, and a deadlock,
under the debugger or not, names the cycle of tasks and locks behind it
with where each lock was taken, which is the fix's first line. A channel
has no holder to name, as any task with a sender may send; its fill is
what it says.

## 2026-11-02 — Breakpoints at events, and frozen tasks

Concurrency bugs live between lines: a task that starts, a value sent, a
wait that lasts. `break on spawn` stops each new task at its first line
of the program's code, after the wrapper `spawn` makes; `break on send`
and `break on receive` stop a task inside the runtime as it uses a
channel, at the program's frame that called it, as a panic's stop does;
`break on wait <ms>` stops a task whose wait for a lock, a channel or a
task lasted longer, when it ends. They are breakpoints with ids, and the
editor sets them as exception filters, as DAP has no events of its own.
`freeze <task>` keeps a task held while the others go on, to let one
side of a race run first; a frozen task that runs stops at its next
line.

## 2026-11-02 — What keeps memory, while stopped

Dumps already find every object of a debug build and what points to it,
from the words of the heap and the tasks' stacks. A stopped program asks
the same: `retainers` gives, for the object a binding or a part of one
holds, the stacks that point to it and the shortest chains of objects
from a stack to it; `memory` counts the objects by where they were made
and says how each place grew since it last counted, which between two
stops shows what accumulates. The editor's console takes them as typed,
with the other commands that only look.

## 2026-11-02 — A type chooses how the debugger shows it

A linked list showed as nested records and a matrix as one long array.
He chose a trait that gives another value rather than a text: a type
that implements `debug.View[T]` shows as the `T` that `view` gives, with
its parts, pages and expressions, and its value as it is stays at hand
as `raw()`. The compiler finds, for the types a debug build's functions
hold, the implementations by their target alone, as the view's type is
what it looks for, and the show glue of such a type calls `view`; a
second glue shows it raw. A view is the program's code, so it runs on
the stopped task, which the debugger's thread asks as `call` does, and
its probes let it run; on the debugger's thread it would wait for
workers the stop holds.

## 2026-11-02 — Captures that do not stop

A server that stops at a breakpoint stops serving, and its clients time
out while one looks. A breakpoint ending in `capture` gives, where its
condition holds and its count says, the stack and the bindings as an
event, and the program goes on, as dynamic instrumentation does in
production tools; `hits 1` takes the first time only. It is a logpoint
that writes what `snapshot` would, and the editor writes it in its
console.

## 2026-11-03 — Conditions compiled in live builds

The debugger reads a condition as it shows values, which is safe and
slow, and cannot call the program's code. In a live build, `tondo`
compiles each breakpoint's condition with a patch, as `call` does, into a
function that reads the bindings it names from their addresses, and the
probe calls it on the task instead of reading the condition. A condition
the debugger cannot read, as a call, stops nowhere until it compiles, and
one that does not compile is an error; one it can read falls back to
reading. Each compiled version keeps the sites of the code it was
compiled with, whose types it knows: a reload that changes the line's
code adds sites the debugger reads, and `tondo` compiles the condition
again for them.

## 2026-11-03 — A value as a table in the editor

An array of records read part by part in the variables view hides what
a table shows at once: which row is wrong. `tondo dap` answers a request
of its own, `table`, with a page of a value's parts, a column for each
field, built from the debugger's `children` without a new command of
the runtime; the extension shows it beside the code, sorted and
filtered in the page, and asks for the next page with *Load more*. The
editor's references name other values after the program goes on, so a
page carries the stop it is of, and one of another stop is refused
rather than mixed with it. Images and charts stay out: a table covers
the values programs hold most.

## 2026-11-03 — PCT for the orders of a test's tasks

`tondo test --schedules` chose the next task at random at each point
where tasks may change places. A random walk finds an order that needs
one task to stop at a given step among many only rarely, and less the
longer the test runs. PCT (Burckhardt et al., 2010) gives each task a
priority as it starts, runs the highest, and at a few steps the seed
chooses drops the task that would run below all, which finds an order
of depth d among n tasks and k steps with a probability of at least
1/(n·k^(d-1)). Two changes give depth three; the scale of the steps they
fall in is the seed's too, so that runs of many seeds cover short tests
and long ones. The algorithm's name in the report changed with it, as a
seed of the old one gives another order. A failure also names the
command that debugs the test in its order, `tondo debug --test <id>
--schedule <seed>`.

## 2026-11-03 — Recording a run, and replaying it

A failure that happens once, as one that depends on the time, the input
or the order of the tasks, cannot be debugged by running the program
again. rr records every call to the system and every switch between
threads, from outside the process, with `ptrace` and the processor's
counters; Tondo's debugger lives inside the process, which `ptrace`
would freeze. So the runtime records: a recorded run puts its tasks on
one thread, in the order of a seed (as `tondo test --schedules` does),
and every function of the C library that reads the world is a wrapper
in a debug build, which `tondo`'s linker gives the program in place of
the function. The wrapper writes what the call gave, its result,
`errno` and the bytes it wrote, and in a replay gives them back without
making the call. Linking them rather than calling them from the runtime
reaches every call, the Rust library's and C's as well; the functions
the Rust library looks up as it runs come through a wrapper of `dlsym`.
The allocator's own calls, the debugger's, and the runtime's threads'
are not the program's, and run in a replay as in any run. What the
program waits for on the host runs on the program's thread while it
records, and signals arrive at the worker's turns, which the recording
numbers. What runs on threads of its own, as tokio's for the network,
cannot be put in that order from inside the process: the recording
marks where such a thread starts, and a replay ends there, saying why,
as it does where the program makes another call than the one recorded.

## 2026-11-03 — What changed between two moments

"What did this request change?" is the difference between two probes of
a replay. `diff` takes the program as it is at each, every task's frames
with their bindings and the memory by place, keyed by the task, the
function and its depth from the bottom of the stack, which stay the same
from one moment to another of the same run, and says what is not the
same. The program stays where it is: the earlier moment is a replay that
stops there.

## 2026-11-03 — Causes across tasks

A value a task receives was made by another: the cause of `let got =
inbox.receive()` is the line that sent it, in its task. While a scan
follows, each value that enters a channel carries the line of the task
that sent it, in a queue per channel, where `try_send` and `try_receive`
move values, the two places every send, receive, `select` and `offer`
pass through; and a task that ends leaves its last line to whoever waits
for its result. A line that received has that line among its causes, so
`origins` goes on in the other task. A value in a channel before the scan
began has no known sender.

## 2026-11-03 — Recordings that travel

A recording is worth most where the failure did not happen: a test that
failed in CI, debugged on a laptop. It had the program and what it read,
but its lines were read from the files where it was recorded. It keeps
the program's sources now, which the DWARF of its debug build names, and
a replay reads its lines from them when the file is not where its path
says; the editor gets the copy too. The directory is what moves, as CI
uploads it; a command to pack it would only call `tar`. Its `run.json`
has the program's environment, so a recording is shared as that is.

## 2026-11-03 — When a condition held

A question about a run is often "when did this first go wrong?". In a
replay, `first` and `last` follow the condition as `timeline` follows a
value, in the frame of its binding, over the whole run, from a replay
that goes to its end, where the program gives what it followed; the
first change to each value is kept, which the last changes may no longer
have. The program goes to the line after which the condition held,
before it runs, as `why` goes to the line that gave a value.

## 2026-11-03 — The origins of a value

`why` answers one step: the line that gave a value, and what that line
read. A failure is explained by the chain, back to what the program read
from outside, which a person asks for one `why` after another. `origins`
asks them all in a replay, from the line of the value, or of the stop,
each line's causes found by a replay that stops at it, while the program
the debugger looks at stays where it is. It gives a tree, each line in
it once, with limits on its size, so an agent gets the whole autopsy in
one answer.

## 2026-11-03 — Following children

A program that starts others, as a server its workers, is debugged in
pieces. With `follow`, the children it starts get `TONDO_DEBUG=1`, which
makes a debug build of Tondo wait before `main` for a debugger, as
`tondo debug` starts one, and leaves any other program as it was; the
debugger hears of each. The editor's adapter asks for a session of the
editor's own for each child, with DAP's `startDebugging`, and that
session attaches: a session is one process, as the editor shows it,
rather than one adapter that mixes the threads of several.

## 2026-11-03 — A `tondo` command in lldb and gdb

dotnet's SOS teaches lldb the runtime's structures. Tondo's dump already
holds what such a command would show, the tasks, their stacks and the
live memory, and `dump read` shows it: so the command, a Python script
for each debugger that `tondo dump plugin` writes, has the stopped
program write a dump through a function of its runtime, which the
debugger calls on the stopped thread, whose task's stack is below the
call, and shows it with `dump read`. It reads nothing of the runtime's
memory itself, so it stays right as the runtime changes. A core file,
which no function can be called in, has the dump `TONDO_DUMP` wrote at
the crash.

## 2026-11-03 — DWARF for gdb and lldb

gdb and lldb read DWARF, and a Tondo executable had none: they saw
symbols without lines. A debug build writes it now, from what the table
of the trace has: a unit with each function, its name and code, and a
line program with a row for each statement, whose probe's code starts
there, and one at the function's start that ends its prologue, where
lldb puts a breakpoint on the function's first line. The compiler writes
the sections in the program's object with relocations to its functions,
and the linker keeps them, the addresses resolved, after the symbol
table. The unit says C, which DWARF has a code for; bindings have no
DWARF yet, and macOS, whose linker leaves DWARF in the objects for
`dsymutil`, has none.

## 2026-11-03 — The editor asks why

`why`, `timeline` and the tasks' waits were answers of the command line
the editor could not ask for. The adapter gives them as requests of its
own, as `table` is, and the extension puts them where a person looks at
a value: the variable's menu and the code's, the call stack's menu for
the tasks, and the lines themselves for what each changed, which an
inline value shows as `then total = 12`. A timeline in a replay is a bar
of the run whose changes take the program there with `seek`. The tasks'
waits are a Mermaid graph, as the modules' are, rather than a view of
its own.

## 2026-11-03 — Recording the network and processes

tokio drives the network on threads of its own, and a child's pipes are
drained by threads too: their calls cannot be put in a recorded order.
A recorded run, and its replay, build tokio on one thread instead, and
the worker drives it between tasks; with nothing to run it waits in
tokio, whose sources include the workers' poller, so a task woken there
wakes it through the poller's own descriptor, a recorded event. tokio
registers the address of each socket's state with epoll, which another
run has elsewhere: the wrappers of `epoll_ctl` and `epoll_wait` keep each
event by its descriptor, and give a replay back what it registered, an
epoll and its copies being one. What tokio would do on its blocking
threads, as looking a host up or reading a file the server sends, the
worker does between tasks; a child's pipes are pipes of tokio, and its
end is its pidfd becoming readable. Children do not run again in a
replay: the program gets what they gave, but what they wrote straight
to the terminal is not shown. `fs.watch`, the sampling profiler and C's
threads still end a replay where they start.

## 2026-11-03 — Going back in a replay

The history `record` keeps is the bindings of each line, as text: it
cannot show a value of the heap it did not keep, nor go on from there. A
replay is the same each time, so going back is running it again to an
earlier probe of its one thread, which it counts, and stopping there with
the whole program as it was. `back`, `reverse` and `why` first run a
replay to where the program is, keeping what they look for: where the
stopped task's lines start, where breakpoints would stop, or the history,
whose lines carry their probe; then another to the probe that gives, which
takes over, with the session's breakpoints. Each stop forks a checkpoint,
a copy of the process that waits on a socket of its own, which those
replays start from instead of the start: the nearest before, or an
earlier one when it does not have what they look for. A breakpoint that
counts its hits counts them from the start, so they start there. `why`
reads the history: the last line that began with another value is the
line that gave the value, in any session that records.

## 2026-11-03 — Checkpoints as a replay runs, and scans that follow

A checkpoint at each stop leaves a long `continue` without one: going
back from its end ran the replay again from the start. A replay that
runs keeps one every second, or every 4,194,304 probes, which it checks
every 16,384 so that the probe stays cheap; 32 at most, the oldest let
go. A scan kept the whole history, every binding of every line as text:
a loop of 12 million lines took minutes and gigabytes. It follows now
what it was asked, the changes of one expression or the runs of one
line, and keeps the last of them. A checkpoint ends when the debugger
that started the replay does, gone or not yet waited for, as one killed
left its checkpoints an hour, each with a copy of the program.

## 2026-11-03 — Why follows a frame, not the innermost one

A value changes where the code that changes it runs: in a method with
`mut self`, a function with a `var` parameter, or another task through a
`sync.Mutex`. A scan that read the expression in the innermost frame
lost it there, or read another binding of the same name. `why` and
`timeline` follow it in the frame that has its binding, found again in
another replay by its task, its distance from the top of the task's
stack and its function, as addresses may differ; that frame lives from
its first probe until its task runs in a caller. A replay runs one
thread, so a change shows at the next probe of any task, and the line
that gave it is the line of the program's code before it, whichever
task ran it.

## 2026-11-03 — Recording the tests that fail

A test that fails once in CI and passes on the next run is the failure
a replay is for. `tondo test --record` runs each test alone in its own
worker, as a recording is of one process, records it, and keeps the
recordings of those that fail, with the tests' executable and the plan
that runs that test: `tondo debug --replay` debugs it as `--test` would,
its frames read as they are there. A recording of a test that passed is
deleted at once, so the directory holds only what is worth looking at.

## 2026-11-03 — Bindings in gdb and lldb

gdb and lldb stopped at Tondo's lines but showed none of its values:
DWARF would have to describe every type and where each binding is at
each instruction, which the debugger of Tondo already knows from its
table, the addresses the probes store and the glue that shows each type.
So the `tondo` command of the plugin asks the program instead, as it
does for a dump: `tondo locals` and `tondo print <expression>` call the
runtime's `tondo_rt_debug_to` with the frame's pc and frame pointer, and
it answers with what `tondo debug` would, its expressions included. For
it to find a frame's slots without a debugger of its own attached, the
compiler writes where each function's slots are in its frame, which
Cranelift's frame layout gives for slots with a key, in the table. A
statement's DWARF row starts after its probe, so a breakpoint on a line
stops where the probe has stored its bindings' addresses, rather than
where they are still the last line's.

## 2026-11-03 — What threads gave, recorded

A replay ended where the program started `fs.watch` or the profiler,
whose threads the recording does not follow. Each is closed its own way.
On Linux `fs.watch` reads inotify itself, in a task of the runtime that
the worker of a recorded run drives, so its calls are the program's
thread's and replay as the network's do; notify stays for macOS. The
profiler's samples come from signals when processor time says, which no
replay repeats: what the program receives of them, the samples of a
profile, goes in the recording as one call, with the frames of the
executable as offsets in it, and the replay gives them back without
sampling; the handler's own calls are not the program's. A child that is
a debug build of Tondo records its own run in the parent's recording,
`children/<pid>`, written as `tondo run --record` writes one, so a replay
of the parent, where the child does not run, names it to be replayed in
a session of its own. Callbacks from threads of C remain unrecorded.

## 2026-11-03 — DWARF in a Mach-O executable

On macOS a linker leaves DWARF in the objects and writes a map of them,
which `dsymutil` gathers into a `.dSYM` bundle beside the executable.
Tondo's objects are temporary and its linker its own: it puts the
program's DWARF in the executable instead, in a `__DWARF` segment that is
mapped with no access, as Go's linker does, with the functions'
addresses resolved and nothing for the loader to rebase. lldb reads it
there, so a debug build is debugged where it is, as on Linux; `--strip`
leaves it out.

## 2026-11-03 — Recording on macOS

Record and replay rest on one idea, that the linker gives the program
wrappers of the C library's functions, and Tondo's Mach-O linker is its
own as the ELF one is: it redirects the same references, to libSystem's
names, as `realpath$DARWIN_EXTSN` and Intel's `$INODE64` variants. What
differs is what macOS calls. `kevent` replaces `epoll_wait`: its events
name their descriptor and filter, so a replay gives each event what this
run registered, where addresses differ. A child's end is a kqueue with
`EVFILT_PROC`, as the pidfd is on Linux. `fs.watch` watches with kqueue
in a task of the runtime, as with inotify on Linux, which leaves no use
for notify. On Apple's arm64 a variadic argument is on the stack, where a
function of Rust does not look, and stable Rust cannot define a variadic
function: `open`, `openat`, `fcntl` and `ioctl` enter through a few
instructions that load it into the next register.

## 2026-11-04 — Threads of C in a recorded run

A function of the program that a thread of C calls ran on that thread, at
once, beside the worker: nothing placed it among the program's calls, and
a callback that waited hung, the only worker being in `pthread_join`. C's
own work cannot be recorded in general: what it writes through a pointer
has no size the runtime knows, so a replay does not give back C's results,
it runs C again. What a replay can hold is the order. One thread at a time
now runs the program's code, whichever has the baton: it lets go in C, in
a wait, at a turn when another asks, and at the end of a callback, and
each move goes in the recording, with the probes, the calls of C and the
way it was let go, which together place it exactly: one place of the
program may let go several ways. A callback that waits works as a worker
on its own thread, and a task that waits with C's frames on its stack gets
a thread that stands in for its worker. A thread that stands in stops by
the task's state, not when the worker sees it, which depends on time. In a
replay threads arrive in another order, and giving a turn to the first that
came deadlocked one that joined the other: each thread is known by which
`pthread_create` of the program made it, and only those the C library
starts by itself by their function and order. A fork loses threads, so `back` runs such a replay from the start. C that
reads the world from its threads is not exact, which only recording every
thread at the level of the system would be, as rr does, and that is
another mechanism, unknown on macOS.

## 2026-11-04 — Crashes say more, and nothing waits for ever

A replay that only looks wrote nowhere, so one that crashed reached the
person as "the program ended". It now writes its errors to a file the
debugger reads when it ends early, and passes on `TONDO_DUMP`. A fault
says what it says outside the recording, and a runtime fault leaves a dump
as the others do. The same silence hid something worse: `exited` runs at
`atexit`, which a fault skips, so a search over the recording of a crash
found nothing, and a crash is what such a search is for. A crash now gives
what the scan followed, unless it holds a lock that needs. A wait for the
baton in a replay used to be endless when the threads of C came in an
order the recording did not have: with nobody running and nothing of the
recording used for 30 seconds it says so and ends, and while recording,
the calls that wait let the baton go, since a callback may be what they
wait for.

## Two recordings compare by the lines each task ran

A failure that happens once in ten runs leaves a good run and a bad one,
and the question is where they stop being the same. `tondo debug --compare`
replays both and keeps the lines each task ran, in a file, since the scan
that gives them runs in the program; then it looks at each task by itself,
because the order between tasks may differ in two runs without meaning
anything, and answers with the earliest line by which a task parts. What
differs at the last line they share comes from `diff`, and where it came
from from `origins`, cut down to the branches that reach a value the other
run got differently at the same line: the full tree was long, and nearly
all of it the same in both. Only runs of one build compare, since the
probes of two builds do not name the same lines.

## Memory and instructions come from the program

An editor asks for bytes and instructions by address, and the debugger is
inside the program, so reading a wrong address would crash what it looks at.
`peek` and `poke` go through a pipe: `write` and `read` answer `EFAULT` where
a load would fault, in Linux and macOS alike, in a container too, with
nothing that a sandbox may refuse. A disassembler reads the bytes in the
tool and not in the runtime, which every program links; yaxpeax, in pure
Rust, adds no C to build, and each target takes only its own architecture.
Instructions are decoded from the start of a function, as the runtime's
tables know it, since x86 cannot be decoded backwards and an editor asks
for those before an address. The first frame is shown where its line starts:
that is where the stop is, and its real address is inside the runtime's
call. Breakpoints on a column are left out: a probe is a line, and a stop
inside one needs a probe per expression.

A long search of a replay is a process of its own, so `cancel` kills that
process and the session stays where it was. The adapter reads the editor on
a thread of its own for that, and only the searches can be cancelled.

## The end of a run is held in copies of the program

A recording is a log of calls, and a log only replays from a state: to drop
its start, something must hold the program as it was at the first call kept.
That state cannot be written to a file and read back without restoring a
process, its C library and its threads included, which on macOS means the
state of dyld and of Mach ports. `fork` already gives that state for the
replay's checkpoints, so `--last` uses the same thing while recording: a
copy sleeps at a probe, the calls after it go to a file of their own, and a
replay is the copy going on with those files. Memory is shared with the
program until either writes it, and the fork takes no time from the run but
copying page tables. The price is that the end of a run lives in processes:
it does not survive a reboot or a container, and CI has to record whole
runs, which is why the calls are compressed too. A copy listens only once it
is stopped and replaying: before, a debugger could reach it while it still
recorded.

## A line stops once, however many blocks it takes

A probe starts each run of code of a line, so a line that a call that may
panic cuts into blocks had several, and a breakpoint stopped at each: three
times on the last line of an HTTP handler. Those probes stay, since `why`
needs them: a value that came from a wait is seen there, on its line, and
not at the next line of another task. What changes is that a probe whose
line goes on from the only block before it says so, and breakpoints, `ran`
and conditions leave it out.

## A copy of the program shows a dump's bindings

A dump is taken when the program is at its worst: memory may be broken and
the thread that crashed may hold any lock. Showing a value runs the
program's glue, and a crash there would lose the dump, so a `fork` of the
program shows them, a line at a time down a pipe, and the dump keeps what
arrives. A copy that crashes costs one binding, as the next copy starts past
it, and one that hangs is killed at three seconds. Text with its parts goes
in the dump rather than raw memory, so that a dump opens with no build and
no knowledge of layouts, on another machine.

## A failed end of a run writes what it held

Copies of the program do not outlive a machine, and CI ends. So a failed
`--last` run writes a report at once, while the copies are there: the last
line, what every frame held, and where what that line reads came from.

## Dumps of optimized code say where the compiler left each binding

The hardest case is a service that fell in production, so a dump shows
bindings without probes too. Cranelift's value labels say where each value
is over the code, and cost no instruction, only a table; the frame layout
says where each slot is, and the unwind information what each function saved
of its caller's registers, which walks them up the stack. Show glue stays in
the binary, 0.5 % of a small one, so a value reads the same as in a debug
build, in the copy that renders them. The inliner keeps which binding is of
which inlined call, since in an optimized build most frames are inlined.

Two limits are the optimizer's. A value that nothing reads later is gone,
and at a panic nothing is read later: so a `panic` the program wrote stores
its numbers in code only that panic runs, which keeps them alive until then.
Bounds checks do not, as they sit in hot loops, and values of the heap do not,
as a dead one may have been moved. Past the runtime's own frames a task that
waits has its registers from the unwind tables the executable keeps: `switch`
left them on its stack, and each frame of the runtime says what it saved, which
a small reader of `.eh_frame` follows in the copy that shows values. macOS keeps
those tables in another form, `__unwind_info`, which says the usual frames of
arm64 and x86-64 in a word each and leaves the rest to `__eh_frame`; the reader
takes both, so a task that waits shows the same there (2026-11-07, found by the
golden test of a dump, which showed fewer values on macOS).

## What is secret is what the program says is secret

A dump of a service has its users' data in it, now that it keeps bindings. The
tools could guess by names like `password`, but a guess that misses leaks with
a good conscience and one that hits hides what someone needed to see. So
nothing hides by itself: `@secret`, on a field or a type, says it, and
`TONDO_DUMP_VALUES=off` takes every value out when no marking is enough.

The mark lives in Show glue alone, so every tool that prints a value gets it at
once, and a part cannot be opened because parts are read from the text. A
newtype shares its representation's glue, so Show of a secret type goes to one
glue of its own. What the mark cannot follow is a plain `String`: a string does
not know whose it is, so a debug dump of a program with secrets leaves out the
text of its objects, and a field's value copied to a binding shows there. That
is why the guide says to prefer the type.

## A dump answers before it is read

A dump of a service with two thousand tasks is data, and someone at three in
the morning wants an answer. So each task's dump says what it waits for and
for how long, which the wait already knew, and `dump read why` adds it up.
The graph goes by who can end a wait, so a task behind a lock leads to the one
that holds it, and that one to what it waits for: the root is what to look at.
A channel has no such edge, as anyone may send on it. The same code reads the
debugger's `tasks`, so `blocked` is this graph live.

## What came before a crash costs what happens rarely

A stack says where a program fell, not how it got there. `tondo trace` keeps
every event, too many to leave on, so a dump's events are only what is rare
enough to read: tasks that start and end, panics, requests, long waits, pauses
of the collector. They go in a list of 256 that only a program with `TONDO_DUMP`
fills. 800,000 spawns ran in the same time with it on, within the noise of
0.3 to 0.4 s, as each is a push under a lock nobody holds for long.

## A call in a replay runs in a copy that ends with it

Asking a recorded run a question should not change the answer to the next one.
A call runs code: it moves the count of probes that `back` and `seek` go by, and
may change what it reads. So it runs in a copy of the process, from a checkpoint
that was there anyway, and the copy ends with it: called twice, a counter says
the same. Recordings are live builds for this, which cost nothing that could be
measured, and a call is then any expression, with what `tondo debug --live` has.
What the code asks of the world is not in the recording, so it reads nothing true.

## Code that is not here is fetched, as if it were

The build knows the repository and commit of everything in it, and the file a
stack points to is often elsewhere: a dependency, or a version that moved on. So
the executable keeps those and a hash of each file, and a tool opens what the
build had, not what happens to be on disk. A stale file with the right name is
the worst case, as it shows wrong lines without a word.

It goes through git, with what the person already has: a partial clone brings
one file, over whatever keys or helpers reach the repository, on any host, where
a provider's raw URLs would need a token and a template each. Templates are only
for links to read. The hash also tells a dirty build, so nothing runs `git status`
at each build, and sources that did not change need no new build for a new commit:
the older commit has the same files. The editor gets a reference, not a path, so
nothing is fetched until a file is opened.

## A dump that stays in a dead container was never written

Most services run where the disk goes with the process, and the kernel's OOM
killer leaves nothing at all. So a dump can leave by a command, which a program
that falls waits for, and a threshold can be a share of the cgroup's limit, which
is the only warning such a death gives. A command rather than an upload built in:
each place has its own storage and credentials, and `aws s3 cp` already knows them.

Many instances fall the same way. A signature goes by the panic's code and the
functions, not its text or lines, as those differ with each value and build.

## Incremental analysis waits, because the numbers say so

It was worth asking again now that the editor does more. A project of 72,000
lines in 61 modules checks whole in 0.16 s with a release build, and an edit gets
its answer in the same time, as parsing and checking already run in parallel.
Reusing parts of an analysis would add invalidation to everything that reads it,
to save time nobody waits for. It comes back when a real project is slow.

## A refactor is what still compiles

Changing a signature, moving a declaration or inlining a call edit text across
files, and text is not the program: a comma in a string, a name passed as a value,
a private helper left behind. Each edit is checked by analyzing the program as it
would be before the editor hears of it, so one that would break it is refused with
the error it would make. That keeps the code that writes edits small.

A hint that a `pub` is for nothing lives in the editor only: in a program it
is a tidy-up, in a library it is a mistake, and `tondo check` has no opinion.

## A logpoint in production writes over a nothing the compiler left

Reading a value at a line of a service that cannot stop needs code at that line.
A debug or live build has it, and runs four to five times slower on code that
computes. Patching any instruction of a Release build costs nothing until used,
but moves code under a running program, and needs single-stepping that aarch64
does not give a process. A check of a flag per line was the safe way: 102 % slower
in a tight loop over an array, as a load and a branch per line add up and the
cold call weighs on registers.

Cranelift has what Wasmtime made for this: a patchable call in the `PreserveAll`
convention, which clobbers nothing, written as `nop`s. That still costs 30 % in the
same loop, where a call is a barrier the optimizer stops at, so a loop that calls
nothing has no sites. Measured on five programs, the rest is noise, under 2 %, and
under 1 % of size. Activating one writes a call over bytes that were put there for
that, with every thread stopped: nothing moves, and no instruction is emulated.

A line in such a loop is refused, saying the nearest that can take one. It would
be hit millions of times anyway, and each hit forks.

## A deadlock is found by the orders that could cause it

A deadlock needs two tasks at the wrong moment, which a test run rarely is. The
orders locks are taken in are there on every run, so each lock taken while another
is held is an edge of a graph, and a cycle is the bug without the bad luck. It costs a
load for tasks that hold nothing. A lock is known by its address, and forgotten when
it is freed, as another may come there. In a test it fails only when asked: the
order may be fine for reasons the graph cannot see.

## A core is read for where tasks were

A program that dies with no dump leaves at most a core, and the runtime is not
there to ask. A core has the threads, which in Tondo are mostly idle workers: what
matters is on the stacks of tasks. Each task starts with a mark while it lives, so
a scan of the memory finds them, without knowing how the runtime's lists are laid
out, and even when those lists are what broke. The stack of a waiting task follows
from what it saved; the lines come from the trace table of the executable, which
has a mark too, as a `--strip` build has no symbols. It turns into a dump for all
that already reads one. Values are read across the runtime's frames by the
executable's frame tables, and shown by the shape of each type the build keeps,
as `Show` writes them and with the same secrets, since a core does not run the
code that shows them. What only code can show, as a view, is left out.

## A copy of a process has one thread, and the memory of them all

A checkpoint is a fork, and a fork keeps every thread's memory and none of the
threads. What the others were doing stays half done, with no one to end it: three
bugs came from that. A result left on the debugger thread's stack, which glibc gave
to the next thread of the copy; the table a copy's own stack still held, so a patch
waited for ever; and a stop of the world nobody was left to end. So every copy that
goes on comes from one place, which holds what the scheduler shares across the fork
and ends a stop, a command of the debugger and a copy wait for each other by flags
the copy can clear, and what a job gives is not on the stack of who asked.

What a thread other than these holds at that moment stays held: locks of the
runtime's other threads, and the allocator's. They are held for moments, and those
threads are idle while a replay runs, so it is not worth a lock of every lock.

## A failure in a dump becomes a test

A dump holds what a function was called with, and a test is that call written down,
so a failure in production can be reproduced at a desk with no guessing. Values are
written as the program shows them, which is what its code reads, and the test is run
before it is kept, so what does not write back is refused and what does is known to
fail the same way.

## What reaches what is read from names

Which tests a change reaches, and what no test or `main` reaches, come from what the
code names: exact for calls, and a superset where a value is passed around, since
naming a function is what reaches it. No run is needed, and an answer that is too
wide costs a test that did not have to run, never one that had to and did not.

## A Release build answers as a debugger would

A breakpoint in a running Release build is a logpoint that stops its task and brings a
dump. Something on the tool's side answers the debugger's protocol from that dump, so
`tondo debug`, the tools and the editor need nothing new, and the red dot and the
variables work as ever. It stops one task, for a bounded time, and does not step:
stopping a service for someone who left is worse than not stopping it.

## A request is followed by inheritance

A task works for the request of the task that started it, and the request keeps what
they did. Nothing is passed by hand and the program does not change. What a task does
after the answer is left out: the timeline is that of the request, not of its tasks.

## Lost updates warn, they do not fail

The runtime sees a read and a later write of a cell around another task's write, not
whether the value written comes from the one read, so a test only warns. The compiler,
which sees that, warns too. Both are on without a flag: the cost is in the tests that
take locks.

## Live is for development

Its probes stay, and its cost is not a goal: what matters is that a change shows when
it is saved. So a frame takes what it passed instead of staying on old code: an entry of
the patch runs the bindings it lacks and a `let` written anew above its loop, and it goes
on where it was. A `var` is state and keeps its value. What runs twice, and what is left
unreleased, are the price, which production would not pay and a developer does.

## A panic in development is a question

Under `--watch` a panic asks for a fix instead of ending the program: what the program
holds is the reason to keep it, and the fix is the next thing that happens. The function
starts again from its caller's statement, because a fix is often above the line that
panicked, and the caller keeps all it has. `tondo run --live` alone still ends: nobody may
be there to answer, and a script must not wait forever.

## What a line left is read from the next

A binding has its new value after its line, so a line's values are those the next line
of its frame finds. Looking costs only while a tool asks, and stops by itself.

## Tondo's tools diagnose Tondo

What hangs in the test suite says why with `tondo dump` and `why`, in the failure of its
case and in the summary of CI, and leaves its dump to open. A hang that took hours to
explain with a core and a script is what a user would get too, so the tools get what
that needed.

The Nightly also sweeps what no single test thinks of: the language server over every
case, whole and cut as writing leaves it, and the viewer over dumps with each field
damaged. The first run found a function without a body ending the compiler, which any
programmer writes on the way to a function, and a wide character ending the server from
three places.

## What is shared is what is named

`dump redact` keeps the fields it names and drops the rest. Dropping what was known let a
new field out: a field added to dumps stays private until someone says otherwise.

## A `String` nobody reads warns

`cli.markup` gives a text, and a program that called it as a statement printed nothing
and heard nothing. A value may be abandoned (11.9), so this is a warning, `W1018`, and
only for what a call makes: a `String` costs memory to make, and one nobody reads was
nearly always meant to be written. Marking functions `must_use` would ask for annotations
the type already gives. The cases and the benchmarks had no such call, so nothing that
was right has to change.

## A table of arrays is the same table

Rows that are arrays go to `cli.table` rather than to a function of their own: one name
to find, and `columns` means what it reads as, the headers, when there are no fields to
name. Strings that are all numbers go to the right like numbers, since a program that
already has text has no other way to say it.

## The command line is in the tools and the editor

What the toolchain does has one place to be added, the command line, and a test holds the
tools and the editor to it, by command and, for the tools, by option. An agent gets a
parameter for each option, since a schema is how it finds them. The editor does not get a
control for each of its hundred: commands for what people do, settings for what they
set once, and *Run a tondo Command* for the rest, which is honest about being a terminal.

## The extension is tested in VS Code

What the extension does through VS Code's API can only be seen there: a command that is
not registered, a task it cannot run. `@vscode/test-electron` runs a real VS Code with the
extension and a suite inside it, with the questions of each command answered by the suite.
Playwright can come later, for what the API does not show, as the values beside the lines.

## Runbooks run

The tools are many and a model finds them by need, not by name: what it lacks is what to
do in each situation. Runbooks say so, as topics of the guide, so that they are also
resources; but a client picks resources, and a model that has not heard of them does not
ask. So the server's instructions name them, and a result says in `next` where it leads,
at the moment it is useful. Their steps are data a test replays, since a runbook that no
longer works is worse than none: the first run of it found a message that blamed the wrong
thing and two steps that were wrong.

## What is published asks for glibc 2.34

A build asks for the glibc of the machine that made it, and so do the programs `tondo`
compiles, through the runtime: built on a recent system, neither ran on Debian 12. So the
builds for glibc are made on Ubuntu 22.04, which leaves 2.34, the version where threads
joined libc. No system on 2.31 has free support left; RHEL 8 and its kin, on 2.28, are
below what any newer floor would reach, and take the build for musl, as `install.sh` does
by itself. A job runs what is published on those systems, since a floor that moves is only
seen by who cannot start it.

## What is internal is where it lives

A package needs to share things among its modules without giving them to whoever depends
on it, and `pub` is all or nothing. A directory `internal` says so with no annotation:
`pub(package)` would be one more thing to write on each declaration and one more level in
the language, and a list in the manifest one more place to keep. It is a rule of imports,
seen on every line that imports, and the standard library is just one more package, which
closes what let a program import `std.internal`. The owner grants this; there is no
equivalent of `//go:linkname`, where whoever arrives takes what they want and the owner can
no longer change it.

## What a review of the language changed

Writing the runbooks showed what gets in the way, and each change here came from a program
that failed. A trait's method takes a dot on a concrete value: the rule asked for what no
other language does. A root module can take `Key` or `Command`, as other modules already
could. A value with no call is an error where nothing uses it, and a call keeps abandoning
what it gives, since without `;` a block's end is a statement. `try { ... }` gives `?` a
local end, with its type from where it goes, as a block that guessed it would make the
type of `first?` decide what the block is.

## What a test warns of, and what a check is asked

Locks in opposite orders cost a test almost nothing, about 36 ns for each lock taken
inside another, so `tondo test` looks at them always, as it does at lost updates: a
warning, since an order that can deadlock may never do so, and `--lock-order` makes it a
failure. A program outside tests pays nothing unless it asks. `tondo check --docs` and
`--performance` are the two profiles 22.8 names. The second warns of a collection named
again in a loop and written after, which copies it on every turn: 0.243 s against 0.002 s
on the loop measured. It only says it where nothing reads the name after the write.

## What a parameter that nothing reads means

An unused parameter of a public function looked like a defect, and going through the
standard library's 23 showed nine that were: `JsonReader.message` lost the path of a nested
error, `jsonValue` did nothing, and the functions the compiler replaces had bodies that
answered with an empty value. The other fourteen are contracts. A trait's default body
speaks for every implementation, so W1003 leaves it alone, and it leaves alone a parameter
that alone gives a `T` its body names, as `reflect.typeOf(value)`. An implementation that
ignores a parameter is still warned of.

Separate traits for what one format needs were the elegant way to shrink `Sink`, but code
written once for every format cannot ask whether its `S` has one: that is specialization,
which 12 excludes. A record and a field go to the format as data instead, in `beginRecord`
and `field`, whose defaults use them to write the map and the key, and only XML overrides
them. The other hooks stay.

## A warning accepted says why

`// tondo: allow W1004: why` keeps a warning out of the list, on its line or the next one
with code. A comment rather than an annotation, since a warning may be on any line. It has
to give a reason, because the authors Tondo is made for silence what they could repair; it
warns when it silences nothing, so that none outlives its reason; and what it silences stays
in `silenced`, to be reviewed. A program with errors may not have reached the warning, and a
profile nobody asked for gives none, so neither counts as silencing nothing.

## A type is built by calling it

`.new`, `Type(...)` and module functions built things side by side, and W1004 found out when
`RateLimiter`, the one of the `sync` family written in Tondo, did not fit. One rule could
go either way: everything `.new`, or nothing. Calling the type is shorter, needs no word, is
what `UserId(5)` and `Int(x)` already were, and what an author coming from most languages
writes. A function named as a type of its module is its constructor; collections take what
they hold, as `Array[Int]()`; other ways to build keep their names, as `sql.open(url)` and
`PrivateKey.fromPem(text)`. `Type.new` is an error with its repair on the language's types
and the standard library's, with no transition, since nobody has programs to migrate; on a
type of a program it is a method like any other.

## What slowness and orders find, and what emulation added

Emulation left the check before publishing: CI runs both architectures natively, so what it
adds is slowness, at over an hour a pass. It stays in the Nightly. `stress` takes its place
locally, on four cores, on one and on a quarter of one, and one core found at once what no
runner had: a held or frozen task kept its worker, so with one worker the program stopped.
`--schedules` left out what needs no wait: nothing changed the order at a `spawn`, at a lock
let go or at a write of an `Atomic`, so 100 seeds gave one order. Those are points too now, and
the cases run in the order of a seed, since what a case says of its output holds in any.

## Models of the scheduler

`testing/models` has the protocols that lost tasks, in TLA+: how workers find tasks and sleep,
and how a task parks. TLC tries every order of their steps, which no test and no seed does.
The first model found what the tests had not: a worker that retires left a task in the queue
and woke nobody, with the last worker asleep trusting one that was leaving, and it takes three
workers to see. A model is written by hand and knows nothing of the code, so what it proves
is the design, and it has to change with `tasks.rs`. The Nightly runs them.

Modelling the rest found more, some by the model and some by reading closely enough to write
it: the stops of the world were not exclusive, so a logpoint could let the threads go in the
middle of a collection; a task in the next of a worker in C, and any timer on one core, waited
for the call to return; and a connection accepted as a server stopped could wait out the grace, which only the model
has shown: a load of clients around a stop never reached it, and with `listen` the program
ends at once anyway. Channels, `select`, joins, groups, locks and parking held, which is worth
knowing too.

## Every order, in Tondo

Rather than TLA+ in the toolchain, its idea in two pieces. A model and the code are two things,
and writing the models showed how they part, so the one that counts most needs none:
`--schedules all` runs the test itself through every choice the scheduler has, which only a
language with a scheduler of its own can give with no change to the program. It goes by fewest
changes first, as most faults need few, and says plainly when it did not finish. `std.model` is
for a design with no code yet: a library, in Tondo, that needs no new syntax. TLC stays for
`testing/models`, for what it has and they do not: a longer record of what it found.
`std.model` says what must happen too, as what breaks that is a loop as often as a state:
`reaches` for what can always still come, and `eventually` for what does, given weak fairness of the
steps named in `fair`. A strongly connected set of states out of the goal is a loop that gives a
step its due if the step is taken in it or is impossible somewhere in it, and if it is neither,
no smaller loop can do better, so one look at each set decides, with no search for cycles.

Going through every choice grows too fast to finish but for a tiny test, and most of what it
runs is the same order again. So `--schedules all` is dynamic partial order reduction: each run
says what its steps touched, and only an order that turns around two steps that touched the same
thing is worth a run, as Source-DPOR does it. Three things decide whether it is sound, and each was
wrong at first. A step must say all it touches, so the mark is in the runtime's lock, which nearly
every object goes through, not at each operation, which could forget one, and at both ends of a
`Mutex`, whose free path takes no such lock and lost an outcome for it; what no address names,
files, sockets, a call of C, counts as one thing, marked by the compiler in test builds so that
Release pays nothing. An alternative must carry the whole sequence that turns the pair around:
forcing only its first step lands in the same order, and that was enough to lose outcomes once
runs of a known order stopped being expanded. And a task that could run and never did says nothing
of what it would touch, so it gets tried. It is held to the plain enumeration, kept behind
`TONDO_ORDERS_EVERY`, on random programs: what the one reaches the other must reach.

## All of LSP but notebooks

`tondo lsp` covers LSP 3.18 but what it says of notebooks, which Tondo does not have. Three
choices shaped it. Answers are written once, for the richest client, and fitted to the editor in one
place, rather than each feature asking what the editor takes: a feature cannot forget. What an editor
resolves later is held back the same way, by moving it out of a finished answer: the server still
computes it, so this saves what goes over the wire, not the work, and making each of thirty refactors
lazy was not worth that. And a cancelled request is only skipped before its turn: requests run in
order on one thread, as an analysis holds state that a second thread would have to share. What
the editor gives up on while it waits, which is nearly all of it, costs nothing; one already running
stops between one project and the next, as the analysis of a project is what the next request needs
anyway. Results in parts are there for the protocol more than for speed: `references` and
`workspace/symbol` read an index in memory, so what takes long comes before them.

`testing/editors/neovim.lua` drives a real Neovim, because the extension can hide what a client of the
protocol needs. It caught nothing yet, and is there so that "any editor" stays true.

The standard modules stay files by default, even with `workspace/textDocumentContent`: a virtual
document is read-only to everything else the server offers, so it would lose hover and navigation
inside them. `virtualStandardLibrary` is there for an editor that cannot read the cache.

## `std.model` for what TLC did, and the code kept in step

`std.model` is to do what we used TLA+ for, and what others may: every property TLC checks, not
only the invariants our models use. Formulas of what comes later are checked as Gerth, Peled, Vardi
and Wolper build them, as an automaton of the runs that break the formula, gone along with the
model; a loop it accepts that gives each fair step its due breaks the formula, and strong fairness
is checked by leaving out, part by part, the states where an owed step is possible. It is held to
TLC itself on random graphs, formulas and fairness that TLA+ can write, and to a brute force over
lassos on those with `until` and `next`, which it cannot: hundreds of cases, all the same. Cases
where fairness changes nothing check nothing of it, so the generator builds rings with ways out,
and dropping fairness must change verdicts for the comparison to count.

A model written beside the code drifts from it, which is the weakness of TLA+. So the first answer
is no model: `model.machine` goes through the production function that gives the next state, with
every value of the event's type, which reflection gives, so that nothing is written but what must
hold. The second is properties of the real code, `model.watch`, in every order `--schedules all`
gives. Only then a model, and `model.mark` in the code with `model.follows` hold the two to each
other in every order: a step one takes and the other cannot fails the test, and a step of the model
the code never takes in any order is said. `mark` costs nothing outside a build of tests: the
compiler writes `false` for whether it records.

A state was first a `Key`, which an `Array` is not, as `Key` implies `Copy`, and writing the 11
models of the runtime showed what that cost: six kept a list of tasks in a `model.Queue` and changed
it through a function of their own. A state is now known by what it encodes to, so it can hold
arrays and maps; of a type that can hold a map, the keys of every object are put in order first, as
a map encodes in the order its keys came, and with the numbers as they are, since the canonical form
of JSON writes them as doubles and two large integers would be one state. That costs a search about
twice the time, 0.14 s to 0.38 s for the 66,058 states of `Lock`. `model.Queue` and `model.Bag` stay
for queues and collections without order. `model.step(name, state)` writes a step, `model.expect`
gives what was checked, and `Violation.property` names what broke, so a test of a bug put back says
which property finds it.

The models of the runtime in `testing/models` are written in both: each `.tla` has a test of
`std.model` that goes through as many distinct states as TLC does, and finds a bug the test puts
back, a lost wake or a leak, which an invariant, a deadlock or a formula of what comes later
catches. `cargo test` runs them, and the Nightly runs TLC on the same models, fails if the counts
part, and holds the two to each other and to the brute force on random formulas. Whether to leave
TLC and Java out once that has held a while is the maintainer's call.

## An error nothing fixes is `Never`

A function that does not fail passes where one that may is expected, `fn(S, E): S` for
`apply: fn(S, E): S ! X`, so that `model.machine` takes a production function whatever its result,
under one name. Two rules make it: a type parameter that only stands for the error of a result, and
that nothing fixes, is `Never`, which is the one answer that fits any call; and a function named
there is the closure that calls it, as the closure written by hand already was. Inference otherwise
still refuses to choose.


## `with` takes a field named alone

`user with { name }` is `user with { name: name }`, as the literal `User { name }` already was. The
two forms differ in nothing but whether the record is new, and the shorter one came naturally in
both: writing the models of the runtime, three updates computed a field's new value in a binding of
its name and then had to spell it twice. A field whose name is a keyword still needs its value, as
in a literal.

## A crash leaves its dump by itself

A program that panicked, faulted or deadlocked left a dump only with `TONDO_DUMP=<dir>`, so the
first crash of a program, the one nobody expected, left its report on standard error and nothing
to read the values from. Now it leaves one by default, values and all, in the cache of Tondo,
`~/.cache/tondo/dumps`: a directory only its user can open, as a dump holds what the program held,
which keeps the latest 20, and whose path the last line of the error output gives. It costs nothing
while the program runs, since it is written when the program falls; what a dump of a running
program needs, as the events and the order of locks, still runs only with a `TONDO_DUMP` given, so
that a build for production stays as fast. `TONDO_DUMP=off` leaves none, and the tests of the
toolchain run their programs so.

## `tondo test` tries the orders of tasks by itself

A test of tasks passed in a plain run whatever order the machine gave, and only `--schedules all`
found the order that loses an update, so a test protected nothing unless CI remembered the flag: a
fresh agent, asked for a test that would have caught a lost visit, wrote one, saw it pass, and
found no way for the test to ask for every order. A plain run now does it for the tests that start
tasks, which the checker marks through what they call, up to 200 orders or 2 seconds each, and fails
one in the order that breaks it. A test that reaches outside the program, files, the network,
processes, a database or C, runs once: running it again would change what is outside, and a test
of a database would fail for its own rows rather than for a bug. Test builds already call the
runtime before each of those, for `--schedules all`, so a plain run notes it at the cost of a store;
an order that reaches outside stops the search too. `tests/orders`, 25 tests of the standard
library's tasks, went from 0.6 s to about 1.3 s. `--schedules 1` runs each test once.

## The search of `--fuzz` counts the standard library too

The search was guided by the counters of the project and its dependencies alone, so a test that
fuzzed what the standard library parses, as `xml.parse`, `url.parse` or a template, searched blind:
nothing it ran counted as new. It now counts the standard library as well, as Go's fuzzing counts
every package, and takes its literals after the project's. In ten seconds, fuzz tests of
`url.parse` and `xml.parse` kept 2 inputs each, as nothing counted; now they keep 119 and 169,
which reach 35% and 43% of what their build counts, at fewer inputs a second, since each goes
deeper. The loop of `testing.fuzz` itself is not counted, nor are the tests, whose code is not
under test.

## A key that is no name of XML is escaped as `_xHHHH_`

`xml.encode` wrote a map's key as the name of an element, so a key with a space, one that starts
with a digit or an empty one gave XML that is not well formed, which `xml.decode` refused: the fuzz
tests of the standard library found it at their first seed. `encode` gives a `String`, with no
error to return, and a panic for a user's data would end a program for its input. So a name writes
each character that cannot be in it, and a `_` before an `x`, as `_xHHHH_`, the convention of
.NET's `XmlConvert.EncodeName`, and an empty key as `_x_`; `decode` undoes it. Names that were
valid do not change, and every map reads back as it was written.

## The runtime reads its variables once, as a program starts

Thirty-seven variables of the environment told the runtime what to do, read in 46 places across
16 files: some once, some each time they were needed, as `TONDO_DUMP` at a crash, and a reader had
no one place to learn what a program could be told (docs/refactor.md, phase 3). They are now the
fields of one structure, `Config` in config.rs, read before anything else as the program or the
library starts, and the table of the toolchain's specification (7.17) lists them, which a test
holds to config.rs, as it holds the runtime to reading the environment nowhere else. A program
that changed one of them with `env.set` before it crashed used to see the change; the runtime now
does what it was started with, as a program that sets `TONDO_DUMP` for itself means it for its
children. Those the runtime takes out of the environment for its children not to inherit leave it
where they did. `TZ`, `HOME` and `XDG_CACHE_HOME` are the system's and are still read where they
are used.

## A benchmark measures without the watches of the tests

`tondo test` watches every test for updates that tasks lose and for locks taken in orders that
can deadlock, and kept watching while `--bench` measured: the watch of lost updates keeps where
each lock of `std.sync` was taken, so a benchmark that takes a lock 4,000 times made 4,051
allocations and took 323 µs, of which 82 µs and 29 allocations were the program's. The comparison
with Go measured the watch, and the speed plan (docs/speed.md) was about to chase allocations the
program does not make. `--bench` now measures the program as it runs outside the tests, as it
already did without the diagnostics of a test that does not end; a plain `tondo test` keeps both
watches, and runs each benchmark's body once under them. `--lock-order`, asked for, still applies
when measuring.

## The standard library's bodies are checked as far as a program reaches them

Every build checked the body of every function of every module it loaded, the standard library's
with the program's, and monomorphization then compiled only what `main`, the tests, the exports and
the contexts reach: of the 8 ms that checking hello world took, 6 went to bodies of the standard
library that it never calls. The checker now checks every body of the program, of its tests and of
its dependencies, which may have errors, and then each body of the standard library that a checked
one could lead monomorphization to: a call or a function value, the implementations of a trait's
method that a receiver of its type may use, all of them for a type parameter, the `Display` of
what is shown, the error of a failing `main`, and what monomorphization calls by name
(`check/reach.rs`). A body nothing reaches keeps its signature, and monomorphization stops with an
error of the compiler naming any such body it is asked to compile, so that an edge missing from
reach.rs fails a test instead of compiling code never checked. The standard library has no errors,
and its test of warnings checks every body of it with `TONDO_STD_WARNINGS`, as an editor's
analysis does to know every name; `TONDO_CHECK_STD` checks them all for any build, and the cases
give the same with it as without. Types are interned in another order, so an executable's bytes
differ from before; what it does does not.

## HTTP, gRPC and TLS run on the workers' poller

Plain TCP stopped hopping from tokio's reactor thread to a worker when the workers began to poll
the sockets themselves (`poll.rs`); HTTP, gRPC and TLS still run on tokio's threads, and every
request crosses to a Tondo task and back: with 64 clients at once `http/tls` takes 136 ms where Go
takes 83 (docs/speed.md, H2). They move to the workers' poller. hyper's connection futures run
inside Tondo tasks, polled in place with a waker that notifies the task, as `std.net`'s streams
wait already; their IO is the poller's sockets, hyper's timer the runtime's timers, and the
futures hyper's HTTP/2 runs in the background are Tondo tasks the server owns. TLS is rustls's
connection without tokio, on the same socket, which also replaces the two tasks that pipe
`std.net`'s TLS today. No callback crosses into Tondo: a task for each HTTP/1.1 connection asks
the native for its next request and calls the handler itself, and HTTP/2 gets a task for each
stream, so `serve`'s single queue goes; the public API does not change. A recorded run records
these sockets through the wrappers plain TCP already goes through. The work goes in eleven steps:
the HTTP/1.1 server, HTTP/2, TLS, WebSocket upgrades, files and uploads, compression, the draining
stop, gRPC's server with gRPC-Web and Connect, the client on hyper's connections instead of
reqwest, gRPC's client, and tokio itself once nothing uses it. Each is measured against Go and
leaves no old path behind.

As built, the first step changed how the futures are polled: hyper's connection is polled by the
workers themselves, at their turns between tasks and right after the poller's events, and a waker
only puts it in their list, because the code that wakes a future may hold a lock its poll takes,
as h2's does; the task of each connection polls it too as it asks for the next request. TLS goes
through tokio-rustls's adapter over the poller's socket, which needs no thread of tokio's; its
own sans-IO connection can replace it if A3 shows the adapter costs.

## The pieces of a split share one block

`text.split(",")` made a string for each piece, a hundred allocations for a hundred pieces, where
Go makes one: its strings are a pointer and a length, which point into the text, and its collector
finds the object a pointer is in. Tondo's strings count their references in a header before their
bytes, so a string that points into another needs a header of its own somewhere. Strings of two or
three words, as Go's, were weighed and left: each copy of a string would be bigger or would look
its owner up in the allocator, on the most common path of the language, and the change touches
every native, literal and tool that reads a string. Instead the pieces of one text go into one
block: each keeps the header and the bytes after it that every reader expects, and its count says
that it is a piece and how far before it the block starts, whose own count holds the references
to all its pieces. A piece is never the only owner of its block, so a change copies it first, as
one of any shared string; an array's release drops the pieces of one block in a row with one
operation. `split`, `lines` and `words` make them: 101 allocations become 2, the array and the
block. What it costs is memory: a piece that is kept keeps its whole block, at most as large as
the text it came from, as a Go substring keeps the text it points into; a piece that changes is
copied first, and the copy keeps nothing else.

## A text only its frame reads is made in the frame

`"{i}".length()` allocated a string and freed it for each number, where Go makes none for numbers
below 100 and one for the rest, which its collector frees in bulk. Escape analysis, as the
maintainer asked of the optimizer on 2026-10-28, starts with texts: a pass over the MIR of a
release build (`stack.rs`) finds the strings that interpolations make and that the function only
measures, compares and releases. Codegen builds each in room of the frame, 56 bytes after a
string's header, whose count says that no one counts it, as a literal's does, so that retaining
and releasing it do nothing; one that does not fit, at the start or as it grows, moves to the heap
and is released as before. Any other read of such a text, an argument, a copy, a loan or a move,
could keep it past the frame, and keeps it on the heap; so does every text of a function that
calls itself, whose frames would each keep room. The room stays under the length from which the
runtime keeps a string's positions by its address, which the room reuses for each text it holds.
Debug and live builds, which skip the MIR's passes, make every text on the heap. `int to text
1000` takes no allocation instead of 900 and 8.2 µs instead of 18.

The value of a closure is made so too when the function only calls it or reads its captures, as
it does once the function the closure was passed to is inlined: its room holds the header of a
function's value and the captures, with the same count, and where it is released the frame lets
go of its captures, which the count would otherwise never free. A closure that another captures,
that is returned, kept, sent or passed on stays on the heap. `closure 1000` takes no allocation
instead of one, and 245 ns instead of 274, where Go takes 426.

A text written to the console or to a socket was made on the heap, as passed to a call that may
keep it, though those calls of the runtime copy it, or send it, before they return: since
2026-11-11 the pass knows them, by name, and such a text is made in the frame. `net/echo 1x1000`
takes 3,000 allocations instead of 4,000, where Go takes 3,744. As the frame keeps room for eight
texts, those made in a loop take it first, which would take the heap in every turn.

A text or a closure made into a named local, as `let label = "{n}"`, lowers to a temporary moved
to the local, and the move kept it on the heap. When the temporary is mentioned nowhere but in
its making and the move that follows it, the pass makes it in the local itself, which then follows
the rules above, as `tondo optimizations` showed it should (2026-11-11). Framing the local beside
the temporary instead would be wrong: a local copied on, as `last = now` in a loop, would still
point at the room the next turn makes another text in.

## `tondo optimizations` says what a release build made of the code

Why a program allocates or checks more than it should was found by reading the MIR the passes
leave (`TONDO_DUMP_MIR`), which takes knowing the compiler. The maintainer asked on 2026-11-10 for
tools that make that easier, for the compiler's work and for anyone who writes Tondo, at no cost to
the code. `tondo optimizations` runs the passes of a release build without generating code and
says, at each place of the program's own code, what they did and what they could not: the calls
inlined and those kept, with why; the texts and closures made in the frame, and what keeps the
others on the heap, followed through the copies to a call, a return, a value or a task; the bounds
and overflow checks dropped, the ones an innermost loop keeps, and the products and divisions done
without multiplying or dividing; and what else is made on the heap at the code that makes it,
arrays whose values are not all constants, maps and sets, slices and the boxes of a type that holds
itself, by the rules codegen follows. What a call allocates inside is told in the called function,
or, for the standard library's, left to the memory profile, which knows the stacks. The passes write a remark only when it is asked for, so a build
does the same work and makes the same bytes as before.

A place is told once, with the functions whose code it is in: its own, and the ones it was inlined
into, where it may fare differently. A loop unrolled or code inlined twice copies a place, and what
is done in one copy and not in another is told as not done when it is a call or the heap, the cost
left; a check that one copy drops and another keeps is told as dropped in one copy, since a
counted loop runs its checked copy only for bounds far from zero. Arithmetic of constants is not told. Each remark has a `kind` for
tools, the done and the not done side by side (`inlined` and `call`, `frame` and `heap`,
`unchecked` and `checked`, `arithmetic`), and its text is English like the rest of the
compiler's, written to follow the code of the place. The MCP tool of the same name gives it to
an agent, filtered by kind or function.

`--against <revision>` builds the code of a git revision the same way, extracted as `tondo test
--against` extracts it, and tells only what changed: what the revision had and what is new. A
place is known by its file, code, kind, text and functions, not its line, so code that moved is no
change. A check kept in a loop is told for every innermost loop, calls or not, so that a call added
to a loop does not read as a check gone.

## A benchmark counts the instructions of a run

A benchmark's time moves by a third when only where its code lies moves, and with whatever else
runs on the machine, so a change to the compiler was judged by `perf stat` on programs written for
it, outside the benchmarks. The maintainer approved on 2026-11-10 counting what does not move:
under `tondo test --bench`, on Linux, where the system lets a program count itself
(`perf_event_paranoid` of 2 or less), the runtime opens the processor's counters of instructions
and cycles outside the kernel on every thread the program has when the samples start, and divides
what they counted over the samples by the runs. The samples are where the time is measured, so the
counts cost nothing more, and the batch that counts allocations, slower for it, is not counted.
A thread started during the samples is not counted, which a benchmark that starts threads of its
own, rare in Tondo, would show as fewer instructions.

On 2026-11-11 the maintainer approved counting, with them, the branches the processor
mispredicts and the reads that miss its first caches of data and of instructions, each left out
where the processor does not count it: when a time moves and the instructions do not, as where code
lies moves it, they say why, and a comparison tells those that moved 5% or more, of an event that
comes once in a thousand instructions or more, as a few misses that come and go say nothing.

The report, the baselines and the reports for people carry the instructions and cycles of a run,
`null` or a dash where they were not counted, and a comparison says how the instructions changed.
`--max-regression` keeps judging time unless `--regression-by instructions` asks for them: the
time is what users feel, and a change that only adds cache misses moves no instruction, so the
choice is the caller's; judging by instructions where a side did not count them fails, with why,
rather than pass a gate that checked nothing.

## `tondo asm` shows a function's machine code under its lines

What a loop costs was read with `objdump` and `perf annotate`, after finding the symbol a build
gave the function, and Cranelift's IR, where the rules of its fork apply, could not be seen at all.
The maintainer approved on 2026-11-11 a command for it. Codegen keeps, when asked, the bytes of the
functions named, the names its relocations give, where each statement's code starts and, with
`--clif`, the IR before and after Cranelift optimizes it; `tondo asm` decodes them with the
debugger's decoder, for this machine, under the lines of the program, each line with what `tondo
optimizations` tells of it. The constants Cranelift leaves after the code end where the last code
with a place in the source ends, so they are counted rather than read as instructions. With
`--against`, a diff of the instructions with a revision's, a jump without its distance, which code
before it moves, and a call by what it calls.

## `--layouts` measures a benchmark in several orders of the code

The same code at other addresses changes a benchmark's time by up to a third, so a change that
moved the functions could look like a regression or a gain it was not. `TONDO_LAYOUT_SEED` showed
it by hand; on 2026-11-11 the maintainer approved `tondo test --bench --layouts <n>`, which builds
the tests in n - 1 more orders, each build kept apart in the cache, and measures each benchmark in
each. Its own measures stay those of the usual order, the one a program runs in, and it adds the
median of each order. Against a baseline measured in several orders too, slower means slower in
every order than in any before, by more than the noise of 2%: a change the orders explain is no
change. Two revisions measured side by side with `--against` would need each built in every order,
so `--layouts` goes with `--baseline`, which is also how two compilers are compared.

## `profile show --asm` takes a profile down to the instruction

A profile says which line takes the time, but a line is a dozen instructions, and which of them is
slow, a division, a mispredicted branch or a read that misses, was read with `perf annotate`. The
maintainer approved on 2026-11-11 `tondo profile show --asm`: the instructions of the function that
took the most, or one named, each with its share. The samples already kept where they were; a
profile now also says where its program was loaded, as pprof's mapping does, so that an address
less that is one of the program's file, and the reader decodes the function from the file by its
symbols, with the debugger's decoder. The innermost frame keeps the address after its instruction,
as the frames that called it do, which the reader takes back. The file must be the one that ran,
and a sample often falls on the instruction that waits for a slow one rather than on it.

`--all` takes every function of the program's own code, and `--compiler <tondo>` compares with
the code another `tondo` makes of the same program, which runs `asm --json`, approved on
2026-11-11: that a change to the compiler leaves the code as it was was proven by comparing
executables, which differ in the date the runtime was built, and which functions a change to a
pass touches was found by hand. Codegen keeps the functions the driver names, by their exact
names, rather than by a part of one.

## `--bisect` finds where a benchmark regressed, with any tondo

`tondo test --bisect` found the commit where a test fails, run by this `tondo` on the code of each
commit. The maintainer approved on 2026-11-11 the same search for a benchmark: with `--bench` and
`--max-regression`, the good commit is measured first and every other compared with it, judged as
a baseline is, so that `--regression-by instructions` makes the search as sure as the instructions
are, where time would take chance for a change. Looking for the change to the compiler that made
a benchmark slower needs each commit's compiler, not this one: `--bisect-with <command>` runs a
command in each commit's worktree that builds the `tondo` to measure with and names it on its last
line, and a commit where it fails says nothing, as one that does not compile.

`--rules` lists the ISLE rules Cranelift applied to a function, approved on 2026-11-11: whether a
rule of `vendor/` matches was guessed from the code it made. ISLE can write each rule that matches
to a log, which costs every rule a check while compiling, so it is a feature of the vendored crate,
`isle-rules`, which leaves out regalloc2's logs that `trace-log` brings, and of `tondo`, `rules`,
for those who work on the code generator: the `tondo` that ships pays nothing and says how to build
one that counts. A logger counts the rules by the function the thread compiles, and a rule is
Tondo's when a comment above it, back to the rule before, says `Tondo:`.

## `TONDO_WAKES=1` keeps each task's last waits and wakes

A program that hangs now and then has most often lost a wake, and finding which one meant
counting with atomics by hand. The maintainer approved on 2026-11-11 a log the runtime keeps when
`TONDO_WAKES=1` is in a program's environment: of each task, its last 16 waits and wakes, which a
dump and `tondo blocked` show, and in every dump whether the wait a task is parked in was claimed,
which, for a task still parked, is a wake lost on its way. It costs a program without it nothing:
it hears the scheduler through the tools' hooks, which a program already pays a load for, in their
cold functions, where a task runs and stops. Which operation claimed a wait is read there too, from
the wait the task resumes in, and who handed a value through a channel with no buffer from the hook
of the debugger's scan, by its mask. Who claimed any other wait is not kept: that is a check where
every wake notifies, which a program without the log would pay.

## An array a loop fills takes its room before the loop

`values.map(f)` made 13 allocations for 10,000 elements, as its array grew by doubling, where Go's
version makes its slice with the length it knows. Reserving in `map` itself, through
`Array.withCapacity`, made its body cost more than the inliner takes, so that `map` stopped being
inlined where it is nested and its closure went to the heap. A pass of release builds
(`reserve.rs`, 2026-11-11) does it instead, for the standard library and for a program's own
loops alike: an array that a counted loop from 0 pushes to once in every turn, and changes in no
other way there, takes room for every turn before the loop, a `Mutation::Reserve` that no source
writes. A loop that may leave before its last turn, by `break` or `return`, or that pushes in some
turns only, as `filter` does, is left to grow, so that an array never takes much more room than
its elements; one that leaves by unwinding or cancellation does not count, as the array goes then.
The room is a hint: the runtime takes it when the system has it, and leaves the array as it was
when not, and grows it to twice its length at least, so that a loop inside another, which reserves
in each turn of the outer one, copies the elements no more often than pushes would.
`Array.withCapacity`, which reserved nothing, takes its room too. `map`, `zip` and `enumerate` of
10,000 elements make one allocation instead of 13, and a program's `for` loop that pushes in every
turn one instead of 13, 5.7 µs instead of 7.5.

## An element of an array a function was given is lent, not counted

A loop over an array of texts or records that a function was given, `for name in names`, held the
array with a reference of its own and each element too: a retain and a release for every element
read, two calls of the runtime and two atomic operations around a measure that costs less. What a
function is given lives, unchanged, for the whole call, which is why `lends.rs` already dropped the
reference of a copy that is only lent; a copy of such a copy lives as long, and so does an element of
an array that does, since nothing releases an element while its array stays as it is. Since
2026-11-11 the pass follows copies of copies, and an element read into a local is such a copy, while
reading an element or a length keeps nothing: an element that is kept, pushed, returned, held past
its turn, captured or given to a task, keeps its reference as before. A function that sums the
lengths of a thousand names it was given runs a third fewer instructions and cycles.

A loop over an array the function made itself, as one is once inlined where the array is the
caller's own local, holds it by a copy the loop retains in the one block that goes to it and releases
only out of it. An element read in the loop and used there alone lives while that copy holds it, so
it needs no reference either; the copy keeps its own, which the order of the blocks shows to cover
every use of the element, though the array's local may be replaced meanwhile. The same sum over
names built in `main` took 34% fewer instructions and 37% fewer cycles.

Copying a field of such an element, or interpolating it, kept the element counted, as any read but
a few did. A copy of a field with a reference of its own needs nothing of the record, and one
without lives only while the record does, which the chain of copies already asks; an interpolation
reads its values and keeps none, but the text it appends to. A loop over a thousand records that
reads and interpolates their fields took 20% fewer instructions and 24% fewer cycles.

## `tondo help` lists the commands by group

With 44 commands, `tondo help` wrote 172 lines of usages, which a person reads past and a model
spends its context on. The maintainer asked on 2026-11-11 for a help to explore instead: `tondo
help` names eight groups, by what their commands are for, each with its commands on a line; `tondo
help <group>` says what each command of a group does, a line each, with the subcommands of `dump`
and the like apart; `tondo help <command>` gives a command's usage as before, and `tondo help --all`
every usage. The groups are a table beside the commands', which a test holds to every command being
in exactly one, and to no group being named as a command, so that `tondo help <name>` means one
thing.

## `--compiler` takes a revision, built once

Judging a change to the compiler meant building the `tondo` before it by hand, in a clone, and
comparing what each made with scripts: the benchmarks' rows, the optimizations, the machine code.
The maintainer asked on 2026-11-11 for the toolchain to do it. `--compiler`, which `tondo asm` took
as an executable, takes a revision of tondo's repository too, in `tondo asm`, `tondo optimizations`
and `tondo test --bench`: its files extracted as `--against` extracts them, and its `tondo` built in
release with cargo, once, in the cache, beside one target for every revision so that a second builds
only what differs; `tondo compiler <revision>` says where it is, for scripts. With `--bench`, the
other `tondo` runs the same tests as asked and saves a baseline, which this one compares with as with
`--baseline`: running both executables in turns, as `--against` does, would hold the runner and the
other's executable to the same protocol, which changes between revisions.

## `--tests` shows what a release build makes of the tests

The benchmarks are tests, and `tondo optimizations` and `tondo asm` read only the program, so a
benchmark's code was copied into a program of its own to be looked at, where it was not quite what
`tondo test --bench` builds: there its body is inlined into the test that measures it. The
maintainer approved on 2026-11-11 that `--tests` read the tests and benchmarks instead, as `tondo
test` builds them, with `--against` and `--compiler` too, which pass it to the revision's and to the
other `tondo`.

## The passes of loops say what they did, in `tondo optimizations`

Whether `unroll` ran a loop four turns at a time, `reserve` gave an array its room before the loop
that fills it, or `lends` read a loop's elements without counting them was found with
`TONDO_DUMP_MIR` and `grep`, as those passes told nothing. The maintainer approved on 2026-11-11 that
they tell it: three pairs of kinds, `unrolled` or `looped`, `reserved` or `grown`, `lent` or
`counted`, each with why not when not, at the loop's range or array, at the push, and at the
element. A loop over a range had no place in the code for its test, which now has the range's, as a
loop over an array's had the array's: on the line of the `for`, which the line tables already gave
the test. A loop only tells of the pushes that are its own, not of those of a loop inside it, which
that one tells.

## `asm --runtime` shows the runtime's machine code

Proving that a change to the runtime costs nothing where it should, as a hook that stays off,
meant disassembling two programs with `objdump` and comparing their functions with a script. The
maintainer approved on 2026-11-11 `tondo asm --runtime <function>`: a function of the runtime as
this `tondo` carries it to link into programs, read from its archive by the function's demangled
name, with what each instruction calls named by its relocation, and with `--compiler` what changed
in it from another `tondo`'s runtime, as `asm --all --compiler` tells it of a program.
