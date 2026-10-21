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
