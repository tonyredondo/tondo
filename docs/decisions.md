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
