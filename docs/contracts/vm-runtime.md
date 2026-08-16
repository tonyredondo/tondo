# Bootstrap VM object and execution contract

**Status:** implemented M3 baseline plus CALL-003 synchronous closure
invocation, CALL-004 effectful-environment retention with execution guards,
OWN-001 intrinsic cursor value semantics, and OWN-002 affine moves/immediate
observations, OWN-003 flow availability, and OWN-004 complete-slot
reinitialization, OWN-005 typed move paths, OWN-006 affine closure captures,
OWN-007 terminal capture obligations, BORROW-001 call-local loan execution,
BORROW-002 last-use regions, BORROW-003 fixed versus structural mutation, and
BORROW-004 static collection-region disjunction, BORROW-005 dynamic overlap
proofs, BORROW-006 shared and exclusive iteration, TERM-003 synchronous
defer/guard
execution, TERM-004/005 structural unwind exclusivity, and GC-001 verified
trace descriptors, GC-002 complete synchronous root lifetimes, and GC-003
cycle recovery under sustained allocation pressure plus GC-004 single
pre-exhaustion collection and atomic publication, REF-001 managed identity
construction/shared content projection, and REF-002 equality and collection
keys by identity, VALUE-001 exhaustive eager logical copies, and VALUE-002
representation-independent copy observations, plus ARRAY-001 runtime array
length, ARRAY-002 checked array indexing, ARRAY-003 checked slicing, ARRAY-004
logical slice snapshots, ARRAY-005 fixed versus structural array mutation,
ARRAY-006 closed lifted arithmetic, and ARRAY-007 named
concatenation/repetition, MAP-001..003 insertion-ordered map operations and
content equality, SET-001 insertion-ordered unique sets and membership, and
RANGE-001 lazy discrete ranges with checked boundaries, and ITER-001/002 static
user iterators plus `for`, `for ref`, `for mut`, and `for var`, NUM-001 exact
intrinsic widths, and NUM-002/005 closed numeric conversion with stable
recoverable errors, NUM-003 fixed-width integer operators, and NUM-004 strict
IEEE arithmetic at each declared precision, plus TEXT-001 immutable UTF-8
strings and TEXT-004 distinct text and byte domains, and TEXT-002
Unicode-scalar String length, indexing, and slicing, plus TEXT-003 static
Display execution and ordered interpolation, plus VARIADIC-001 homogeneous
final packs and VARIADIC-002 explicit Array spread, plus OPT-COW-001..003
measured and differentially validated collection copy-on-write, plus the
canonical inferred-suspension `suspends` effect and implicit-await path (the
explicit-await prototype remains only in the frozen corpus), EXEC-001/002,
SCOPE-001, SPAWN-001, JOIN-001,
CANCEL-001/002, PANIC-ASYNC-001, SEND-001, SHARE-001, and MAIN-ASYNC-001

**Language baseline:** Tondo 0.1

This contract fixes the bootstrap object model selected by DEC-006. It is an
implementation boundary, not a source-visible memory layout or a promise for a
future native ABI.

## Values and identity

The interpreter uses an explicit `Value` enum. `Unit`, booleans, integers,
floats, individual `Byte` values, characters, function identities, and
structured `Join` handles are immediate values. The canonical `std.bytes.Bytes`
and `BytesBuilder` values are typed opaque host tokens; their payload never
crosses the source-visible runtime boundary. A managed value is a generational handle into a
non-moving heap slot; source code cannot observe the slot index, generation,
address, or collection schedule.
The enum also has one VM-internal `Loan` carrier containing a lender frame,
fixed place, and mode. It exists only in a borrowed callee parameter and is
rejected by copying, snapshots, heap storage, host conversion, and every public
runtime boundary.

Managed heap objects cover:

- strings;
- tuples and arrays;
- insertion-ordered maps and sets;
- concrete closure environments with ordered optional capture fields;
- newtypes, records, enum variants, options, results, and union injections;
- ranges and lazy iterator state; and
- `Ref[T]` identity cells with one present traced payload.

A checked numeric conversion allocates an ordinary `Result`. Success contains
the target scalar; failure contains exactly one zero-payload intrinsic
`NumericConversionError`. Float-to-integer failures are classified in the
observable order `NotFinite`, `NotIntegral`, then `OutOfRange`; integer
narrowing and finite `Float64` to overflowing `Float32` use `OutOfRange`.
Identity and total conversions never allocate a result and a failure on either
verified path is a VM invariant violation, not a source-level fallback.

Integer `+`, `-`, `*`, `/`, `%`, and signed negation check the exact destination
width and produce `P0005` on overflow. Division and remainder by zero produce
`P0003`; signed minimum divided by `-1` overflows while its remainder is zero.
A shift count outside `0..width` produces `P0010`. A valid left shift masks back
to the operand width instead of raising overflow, and right shift is arithmetic
for signed integers and logical for unsigned integers and `Byte`. Bitwise
operations and compound assignments preserve the same fixed-width patterns.

The VM stores scalar floats in one `f64` envelope, but the bytecode type remains
authoritative. Every `Float32` input boundary and every `Float32` arithmetic
operation converts through native binary32 before returning the exact widened
value to that envelope; `Float64` operates directly in binary64. Consequently
rounding occurs at each source operation, subnormals are not flushed, signed
zero and IEEE comparisons remain visible, and a multiply followed by an add is
two operations rather than an implicit FMA. Named constants use the same
canonical envelope representation, including constant infinities and NaNs.

Every managed string contains one host `String`, so allocation, constant
materialization, host conversion, logical copy, and GC tracing preserve valid
UTF-8 by construction. Source equality and ordering compare exact Unicode
scalar sequences without normalization, membership observes one `Char`, and
string iteration yields those scalars in order. The intrinsic cursor stores a
UTF-8 byte boundary internally and advances by the yielded scalar width, making
a complete traversal linear while keeping byte offsets unobservable. Logical
copies may share the immutable object because no source operation can mutate
it or observe its identity.

String length, indexing, and slicing first count or collect Unicode scalar
values; byte length never enters source semantics. Index normalization is the
same mathematical operation used by arrays and invalid positions produce
`P0001`. Slicing calls the same normalizer with the scalar count, including
sign-dependent defaults, clipping, omitted negative-end behavior, and the full
`Int` range; step zero produces `P0002`. The result is a newly materialized
valid UTF-8 String in the bootstrap, an unobservable choice that may later
become a shared contiguous view. A non-unit stride necessarily constructs the
selected scalar sequence.

`String`, `Char`, `Byte`, and `Array[Byte]` remain distinct runtime and bytecode
types with no implicit conversion. The language core has no intrinsic `Bytes`
type; that name and any explicit text encoding or decoding API belong to the
future standard library.

Intrinsic scalar `Display` consumes the same verified shared call loan as an
ordinary implementation. `String` returns its immutable logical copy; `Unit`
uses `()`, booleans use `true`/`false`, integers and `Byte` use decimal,
`Char` emits its scalar, and each float uses the shortest spelling for its
declared IEEE precision. These spellings are the deterministic bootstrap
formatting bridge; the public formatting API and its final contract belong to
the core standard-library specification.

Interpolation evaluates all already-converted `String` operands in source
order, preflights the total UTF-8 byte length, reserves fallibly, validates the
final Unicode-scalar length, then allocates one valid UTF-8 result. Every
operand remains a precise temporary GC root through publication. Segment/value
arity and types were independently verified, and allocation failure is a
resource error rather than a partial string or language panic.

An array object owns one ordered vector of optional value slots. The vector
length is the array's runtime length; it is not duplicated in bytecode type
metadata. Construction fixes that count from the evaluated operands, and
logical copy, argument passing, and return preserve it with the complete value.
The internal `Length` observation counts vector slots for an Array or Unicode
scalars for a String after verified typing. It currently serves language
semantics and does not choose the name or shape of a future standard-library
length API.

All array index paths call the same normalizer: value reads, projected reads,
moves, writes, place validation, loan-region resolution, and constant
evaluation. `0` through `n - 1` remain unchanged; `-1` maps to `n - 1` and
`-n` maps to zero. Empty-array access, `n`, `-n - 1`, `Int.min`, `Int.max`, and
every other invalid value produce `P0001 bounds` at runtime without an
overflowing intermediate. A simple assignment evaluates its index once and its
complete RHS before bounds validation, then performs no write if validation
panics.

All runtime array-slice reads and projected place/loan paths call the same
normalizer as constant evaluation. Omitted bounds retain their sign-dependent meaning;
only explicit negative bounds are offset from the current runtime length and
then clipped. Both stride signs, empty arrays, and the full `Int` range use
distance-before-advance checks, so `Int.min` is a valid negative step rather
than an overflow case. The selected ordered indices back reads, writes,
borrowed paths, and overlap proofs uniformly. Step zero produces `P0002`
before a callee or store can observe the slice.

Once normalized, both direct materialization and a by-value read through a
borrowed slice place call one `copy_array_snapshot` routine. The bootstrap
allocates a distinct outer array and applies the exhaustive logical copier to
each selected element. Nested ordinary values are therefore independent;
immutable strings may share storage and `Ref[T]` retains identity exactly as
for any other logical copy. This eager strategy is not observable and can later
become COW without changing the tests. A `ref`/`mut` slice argument bypasses
materialization and continues to resolve against the lender's original region.

Named concatenation and repetition share one `ArraySequence` execution path.
It evaluates and roots the shared receiver before the value argument, validates
the complete mathematical result length, reserves one fresh compact outer
array, and applies the same exhaustive logical copier to every source element.
`Concat` appends the right sequence after the left. `Repeat` emits ordered
copies, returns an empty array immediately for zero or for an empty receiver
with a nonnegative count, maps a negative count to `P0011`, and maps a result
length outside `Int` to `P0005`.
Valid lengths that cannot fit the configured heap remain VM resource
exhaustion. Nested ordinary values are independent, while immutable String
storage and `Ref[T]` identity keep their normal sharing rules.

Closures pair a concrete bytecode callable identity with a managed environment
whose capture fields use the same optional-value move representation as other
aggregates. The environment traces every present capture and is rooted by the
closure value in a frame, another object, or the operation-local root stack.
Logical copy recursively copies the environment and every Copy capture;
immutable strings and `Ref[T]` retain their ordinary sharing rule. Snapshotting
produces the detached callable identity plus detached capture values for
tooling. Sync, unsafe, suspendible, and unsafe-suspendible closures share this storage
machinery; their exact effects remain in callable type metadata, not object
layout.

Compound payload fields are individually optional internally. Absence records
a logical move and is never a Tondo `none`. Bytecode verification and runtime
checks prevent an absent field from being observed as a value.

Tondo value semantics do not expose physical sharing. The VM retains an eager
reference strategy and uses copy-on-write by default for eligible `Array`,
`Map`, and `Set` buffers. Immutable strings and identity-bearing `Ref[T]` cells
continue to share their managed object because that sharing is itself the
language contract.

The eager walker recursively copies tuples, arrays, maps, sets, closure
environments, newtypes, records, all enum payload shapes, options, results,
unions, and ranges, retaining completed children as temporary roots until the
new object is published with the source descriptor. COW replaces only a
top-level collection-buffer traversal whose shallow values are representation
safe: every stored array element, map key/value, or set key must be scalar,
immutable `String`, or `Ref`. A compound value keeps its own wrapper, and
recursive copying may independently share eligible collection leaves.
Consequently no copied logical path aliases a mutable record, closure
environment, nested collection wrapper, or affine owner.

Collection buffers are immutable `Arc<Vec<_>>` values until mutation.
`is_unique` observes physical ownership; a write uses `Arc::make_mut`, and the
heap records a detach only when the replaced object still had another physical
owner. Heap byte limits conservatively charge each logical collection owner for
its complete capacity. That preserves the pre-COW worst-case bound and ensures
a later detach needs no unaccounted capacity, even though current physical
memory may be lower.

Copying an admitted intrinsic cursor recursively copies its owned source (or
duplicates its shared reference), preserves the current index, and allocates an
independently advancing iterator object. The eager and COW strategies run the
same stable comparison corpus. Its oracle observes only values, post-write
independence, identity, iteration, panic, output, exit status, and survival
under GC pressure. Heap handles, allocation counts, collection timing, and
storage strategy are excluded from that oracle.

Constructing `Ref(value)` allocates one cell and transfers the already
evaluated payload into it. Copying the resulting `Ref[T]` copies only its
generational heap handle and performs no allocation or payload copy.
`RefValue` reads the present payload but has no runtime take or write arm;
bytecode verification rejects those access forms before execution. Equality
first compares handles, so aliases are equal, while two distinct `Ref` objects
short-circuit to unequal without inspecting content. Maps and sets call the
same equality routine for keys, preserving identity semantics without exposing
an address, slot, generation, or collector schedule.

Advancing an own cursor is a destructive ownership transfer from that private
state. Arrays and sets remove the next element; maps remove a key/value pair and
yield a newly allocated tuple. The cursor retains the compact remainder, and
the yielded managed value is rooted while iterator state is replaced. Ref
cursors retain their shared source and expose only a verified position into it.
Mut cursors retain an exclusive source loan and the same position-only state.
Array element writes resolve through that position; map writes resolve through
the value half of the current entry rather than through a temporary yielded
tuple. `mut` replacement validates equal structural extent before publication,
while `var` may replace the current value. Neither path changes the traversed
collection's length, keys, or order.

Set construction compares each evaluated key with the ordered prefix already
accepted, keeps the first equal key, and never changes that position for a
duplicate. Membership uses the same equality and set equality matches members
independently of insertion order. Range objects retain their exact endpoints
and endpoint kind. Integer iteration computes the current mathematical offset
without adding one to an emitted inclusive maximum. `Char` iteration advances
only through Unicode scalar values, jumps from `U+D7FF` to `U+E000`, and marks
an inclusive `U+10FFFF` exhausted immediately after emitting it.

## Frames and roots

Execution uses an iterative Rust vector of frames; a Tondo call never recurses
through the Rust call stack. Each frame owns:

- one function identity that selects its immutable VM-derived trace descriptor
  and exact typed slot schema;
- the verified bytecode function, block, and instruction cursor;
- one state per typed slot: dead, live-uninitialized, or live with a value;
- one optional normalized reservation per function-local loan identity; and
- one ordered stack of captured deferred operations, each with a lexical scope
  and optional affine guard place;
- one stack of active runtime task-scope identities; and
- an optional normal/unwind continuation for its caller.

Parameters and the return slot follow the function metadata. Explicit
`storage_live` and `storage_dead` instructions control scoped temporaries;
function-wide slots start live. Reads, writes, and moves check their runtime
state even though the bytecode verifier has already proved the same contract.
`Move` takes the complete value or projected payload from its slot; `Borrow`
performs a shallow read only in a verifier-approved immediate operation and
cannot become a stored runtime reference. OWN-003 proves source-level
whole-binding availability. OWN-005 makes the bytecode verifier independently
track typed unavailable move paths across sequential, branch, and loop paths,
rejecting repeated, ancestor, descendant, and conservatively overlapping
moves. The runtime still checks each take defensively and represents a moved
aggregate component as an absent internal slot that source code can never
observe directly.

For assignment validation, an unprojected write path consists only of the slot
identity and can be resolved while that slot is uninitialized after a move. The
eventual store installs the new value. A projected path still reads and walks
its aggregate root, so a field, index, or slice write cannot revive a moved
aggregate. Read validation used by compound assignment also continues to
require the direct slot's current value.

The immediate observation subset executes equality, membership, length,
discriminant, index/slice-base, indirect-callee, and slice-shape borrows.
`Borrow` remains a shallow read and never creates a runtime reference.

`ReserveLoan` resolves its fixed place to a normalized `(frame, slot, path)`
identity after all projection operands have been evaluated. Shared/shared
overlap is accepted; every overlap involving `mut` or `var` is rejected. A call
then consumes each reservation exactly once and installs the VM-internal loan
carrier in the callee's corresponding parameter slot. Reads recursively reach
the lender place. Writes through `mut` or `var` update that original place,
including nested reborrows and fixed field/tuple projections; writes through
`ref` and moves through every borrowed parameter are invariant failures.
Before a root write through `mut Array[T]`, `ensure_mut_array_extent` reads the
effective lender, compares its logical length with the replacement, and
publishes only when they match. It roots the replacement across that read
because a sliced lender may allocate an eager snapshot and trigger GC.
`var Array[T]` bypasses this equality check and may change the length of a
complete owner; a slice can never supply that mode.
Reborrow strength is checked both by bytecode verification and defensively at
runtime through the same `BytecodePlace` classification. In particular, a
`var` reborrow from `mut` must end in a complete structurally replaceable
subplace; roots, slices, array rests, potential map entries, and opaque
projections are rejected before a reservation is installed.

An index or slice loan reaches `ReserveLoan` only after a checked
`ValidatePlaces` edge has normalized its bounds and step. The verified static
proof may admit disjoint index, interval, stride, array-prefix, and array-rest
paths without an overlap opcode. Execution still resolves the actual path and
defensively compares it with every active reservation, so malformed metadata
cannot turn the proof into aliasing. Bounds and zero-step failures enter normal
language unwind before the callee starts; any earlier argument reservations in
that frame are discarded during propagation.

`ReleaseLoan` removes a reservation when later argument evaluation takes an
early control transfer. Normal return rejects any reservation left active.
Language panic clears current-frame reservations before entering unwind, and
each propagated unwind clears the abandoned caller frame before following its
cleanup edge. Because the synchronous caller frame remains live throughout the
call, the original slot also remains a precise GC root. Host callables cannot
declare or receive borrowed parameters in the bootstrap ABI.

Registering a defer evaluates and snapshots every `Copy` operand immediately.
Each completed snapshot remains on the operation-local root stack until the
whole entry has been installed in the frame, including while later operands
allocate and trigger collection.
Its one optional affine operand is represented by a guard placeholder and stays
in the verified frame place until cleanup. A retarget changes only that place;
a disarm removes the complete entry. Both operations defensively require at
most one matching entry. Deferred snapshots and guarded places are precise GC
roots. `DrainDefers` repeatedly removes the last entry belonging to any
abandoned scope before invoking it, so an action is disarmed before it runs and
cannot execute twice. The guarded frame place is consumed by that invocation;
verified code cannot access it again unless a complete write reinitializes it.

At every possible collection, roots are enumerated precisely from every live
value in every frame of every task, including parked tasks, captured values in
their explicit cleanup entries, completed child return values awaiting
consumption, and one explicit stack of operation-local values that have not yet
been published. A store, frame push, cleanup registration, task completion, or
heap publication transfers reachability to that owning container. Move,
`storage_dead`, cleanup removal, completion consumption, frame pop, and a
temporary-scope marker withdraw it. Every allocation-capable path restores its
marker on success and VM error.

The operation-local stack covers completed constant and host-result children,
left-to-right operands, dynamic map entries, record updates, assertion parts,
recursive copies, projection and slice copies, nested array arithmetic, named
array sequences, variadic packing, and call preparation. Every completed
sequence element remains rooted until the fresh array is published, including
when a later recursive copy triggers collection. The object being allocated is
traced as a pending object during the same collection, so a completed parent
does not need a second publication step. Moving an affine array rest takes its
contiguous elements into a new owning array, leaves holes in the
compiler-owned scrutinee, and roots both parent and moved children across the
allocation.

Terminal fallback traversal is structured runtime state outside a frame slot:
the removed owner and every child queued for reverse-order teardown remain on
the same temporary-root stack until traversal completes or fails. Object
replacement enumerates those roots too. Closure environments require no
parallel root registry; they are ordinary managed objects reached through a
closure value in a frame, cleanup, temporary, or another object.

The synchronous host ABI has no handle container. It receives detached,
recursively owned `RuntimeValue` snapshots, so retaining a host argument does
not retain its former VM object. Returned snapshots are materialized under a
temporary scope until their full managed result is published. A task suspended
on a child or scope stores the same frame vector in its task record; it remains
a root source but cannot cross the host boundary.

For nominal host results, the stable boundary uses the public type name plus
record values in declaration order, or the public enum name plus a zero-based
variant ordinal and payload values in declaration order. Source-local symbol
and member IDs are forbidden at this boundary. Materialization resolves that
shape against the already verified nominal descriptor, checks the public name,
kind, arity and every recursive child type, and keeps completed children rooted
until the enclosing value is published. Bytecode retains the fully qualified
canonical identity separately for uniqueness; it is not a host wire name.

Bytecode admission derives one immutable frame descriptor per function and
checks that its slot vector exactly matches the verified function. Pushing a
frame repeats that identity, count, and type check before any slot can become a
root. Live slots are still inspected through the tagged bootstrap `Value`
carrier: a function-typed slot can contain either an immediate named function
or a managed erased closure, so static type alone is not a safe root bitmap.
The descriptor instead proves which schema is being interpreted. A suspended
frame retains the same function identity and slot representation, selects the
same descriptor without copying its schema, and is enumerated from its task
record. Suspension therefore adds an owning container, not a second frame
layout.

## Collector

The bootstrap collector is precise, non-moving, stop-the-world mark-and-sweep.
It has no finalizers and can reclaim unreachable cycles. Heap handles contain a
generation, so reuse of a reclaimed slot cannot make a stale handle valid.

The VM independently derives a closed trace catalog from admitted bytecode. It
contains one descriptor for every type and covers strings, tuples, arrays,
maps, sets, closure environments, newtypes, records, variants, options,
results, unions, ranges, cursors, and the `Ref[T]` cell. Opaque results
reuse the concrete witness shape. A generated closure environment must belong
to exactly one callable and retains its ordered capture schema. Intrinsic
arities, referenced types, nominal layouts, frame slots, duplicate
environments, and cyclic opaque representations are validated while deriving
the catalog.

Every heap slot stores the type ID of the descriptor under which it was
allocated. Allocation and replacement first prove that the runtime object
matches that descriptor: object family, aggregate arity, nominal or callable
identity, field/member order, variant payload shape, union member, and cursor
mode are checked where applicable. Marking then follows only the present edges
authorized by that same descriptor. A rejected replacement leaves the previous
object intact. Copying an object preserves its original descriptor, including
through callable erasure and opaque representation boundaries.

`Join[T,E]` is an immediate VM-only pair of child-task and owning-scope
identities, not a heap object and not a public address. `Pointer[T]` has no
constructible managed bootstrap representation. `Command`, `Pipeline`, and the
opaque process values are typed run-local host identities rather than managed
heap objects; their payloads and live child resources remain in the hosted
registry. Any later managed runtime shape must extend the sealed catalog; an
ad-hoc object-side tracing method is not an extension point.

Allocation may request a full collection when the object threshold, byte
budget, or slot budget is approached. The object being allocated and all of its
children are temporary roots for that collection. Growth of an existing object
uses the same rule and protects the target handle internally, independently of
the caller's root list. Capacity arithmetic uses checked addition. Each request
may initiate at most one complete collection; capacity is then evaluated once
more before either one publication or VM exhaustion.

The bootstrap can detect a logical budget failure before attempting to mutate a
slot. Its normative retry is therefore the post-collection capacity check
followed by the single publication attempt, not a deliberately failing partial
insert. A rejected allocation never enters the heap or increments allocation
statistics. A rejected replacement leaves the target descriptor, object,
generation, and per-slot byte accounting unchanged. The collection itself may
still reclaim unrelated unreachable objects and update global accounting and
the free list, which is not program-observable.

The runtime test build has a private memory adapter over these exact allocation,
replacement, root, descriptor, and pressure paths. It can connect managed nodes
without exposing a source operation. Its mixed graph
`Ref -> Array -> Closure -> Ref` proves that one published root preserves the
whole cycle through repeated collections, that peer cycles without roots are
reclaimed on every pressure round, and that withdrawing the final root makes
the retained cycle reclaimable too. The adapter never calls a separate test
collector or marks slots directly.

REF-001 reuses this exact collector path: public construction allocates the
already catalogued cell, its descriptor traces the payload, and every copied
handle reaches the same object. The source-visible `Ref[T].value` projection is
read-only, so it cannot create a back-edge by mutation. The private runtime
adapter remains the conformance path for cyclic graphs that safe source cannot
construct directly.

Object and byte accounting uses saturating checked budgets. Collection order,
free-list order, slot addresses, and threshold timing are not observable Tondo
semantics.

## Cooperative tasks and structured scopes

The executor is single-threaded and cooperative. One task record owns a frame
vector, pending abnormal state, one of `Running`, `Runnable`, `Waiting`,
`Complete`, or `Consumed`, and its parent-scope identity. The FIFO runnable
queue gives each admitted task one bytecode step before requeueing it. A
separate `queued` bit makes enqueue idempotent; a wake only changes
`Waiting -> Runnable`, preserves the exact suspended continuation once, and
ignores duplicate or stale notifications. If no task is runnable before the
root completes, the VM reports an invariant failure instead of spinning.

The selected safe entry may be synchronous or suspendible and always becomes
task zero. This runtime root is not a lexical `scope`, so it does not authorize
detached `spawn`. A direct suspendible call, whether written with `await` or
not, pushes the callee frame into the same task and resumes the caller with its
logical value. `spawn` instead creates a new runnable task under the active
innermost task scope and stores an immediate
`Join` in the owner. Awaiting an incomplete handle consumes no state yet: the
owner parks with its exact handle place, destination, normal edge, and unwind
edge. Child completion wakes it; resumption then consumes both handle and
completion exactly once.

Each runtime task scope stores its source identity, owner task, children in
creation order, and closed bit. Draining requests cancellation for every live
child and parks the owner until all children are complete. Cancellation is one
internal `RuntimeUnwind::Cancelled` state, distinct from returned Tondo values,
recoverable error `E`, language panic, and VM failure. It is observed only at
the implemented cooperative boundaries: an implicit or explicit wait, `spawn`,
task-scope entry, and task-scope drain. The child then follows its ordinary unwind ledger, including
defers and nested task scopes, before completing as cancelled.

A child panic requests cancellation of its live siblings and wakes the scope
owner. The owner still waits for every child cleanup, then selects the first
unobserved child panic in creation order and appends later panics as suppressed.
If the owner was already unwinding from another panic, the child panic is
suppressed under that existing primary. Task-scope teardown consumes any
remaining `Join` fallback recursively before marking the scope closed. Root
completion defensively requires every non-root task to be consumed, so no child
can survive its owner even if malformed bytecode passed an earlier check.

Explicit cleanup entries are drained from the same runtime LIFO ledger. A
`defer await` entry is captured before the scope continues, then its suspendible call
is started only when that entry reaches the drain. A bytecode callee reuses the
ordinary frame continuation; a suspendible host call parks the current task in a
dedicated deferred-host wait state. That wait is not cancelled by the unwind
which started the cleanup, and completion resumes the same drain block so later
entries and the original panic/cancellation retain their precedence. Ordinary
`defer` entries still reject suspendible host results defensively. A cleanup panic
continues through the remaining LIFO entries and is recorded as primary or
suppressed according to the existing unwind rules.

## Control flow, calls, and panic

The VM executes verified branches, tag dispatch, loops, iterators, calls,
returns, and cleanup edges directly. Checked operations either produce a value
for their normal successor or begin a language panic on their unwind successor.

An array sequence evaluates its receiver and argument through that same checked
operation discipline. Only after both complete does preflight inspect the count
or the two runtime lengths. It performs no result-element copy before detecting
`P0011` or `P0005`; an operand panic therefore retains precedence. Allocation
failure remains the third VM/toolchain-error channel and is never relabelled as
a language panic.

An indirect call evaluates and roots its callee before evaluating arguments
left to right, retaining every completed value as an operation-local root. A
uniform named function selects its direct implementation. A managed closure
selects the callable stored in its environment and inserts that same environment
as hidden parameter zero before pushing the body frame. `Borrow` performs a
shallow read so `Call`/`CallMut` bodies observe the original environment;
a Copy-based `CallOnce` logically clones the environment before invocation,
while a Move-based `CallOnce` takes the closure owner and passes its existing
environment. Moving an environment capture takes that optional field and leaves
it absent, exactly like any other verified aggregate projection. The bytecode
verifier has already proved the exact signature, protocol, access, and move-path
combination, so runtime dispatch performs no trait selection. Opaque callable
views and closure-to-`fn` erasure are representation-preserving and still reach
the same managed closure value.

Call preparation collects every verified `VariadicElement` value in textual
evaluation order and publishes one fresh `Array[T]` in the callable's unique
final variadic parameter slot. No elements produce an empty array. Each
operand has already taken the ordinary contextual path: Copy elements are
logical copies and affine elements are moves, so the pack introduces neither
aliasing nor a second copy. The bootstrap materializes eagerly; this allocation
choice is unobservable and may later become a temporary view without changing
the body-visible immutable `Array[T]`. Completed elements and the pending array
are operation-local roots, including while nested values or closure
environments trigger collection.

A verified `VariadicSpread` is likewise the unique final argument, but its
operand is the complete `Array[T]`: contextual access has already produced a
logical Copy snapshot or moved the affine owner. Call preparation drains that
temporary array into the pack without invoking the logical-copy walker again.
The source remains available only in the Copy case; an affine source is
unavailable after the call. Named spread has the same runtime path and differs
only in its verifier-proved association with the exact variadic parameter.

The ordinary call path admits synchronous signatures and suspendible signatures.
For a suspendible signature, the verifier lowers an ordinary call to the same
`Await` operation used by explicit `await call()`; only `Spawn` preserves a
pending handle. The verified `unsafe_call` bit must agree with the callable.
The public execution entry accepts a safe sync or suspendible body and rejects
an unsafe root; nested unsafe execution has already crossed a verified lexical
region. Effectful closures retain their exact type through
construction, copying, tracing, snapshots, and erasure. Raw Pointer operations
cross only their distinct typed privileged-host boundary; the bootstrap exposes
no allocator, stable layout, or safe address source and therefore invents no
native semantics without a pinned adapter.

A panic stores its normative `P` code, stable name, message, primary source
span, and a canonical innermost-first call stack. Cleanup blocks execute while
the pending panic crosses frames. A panic raised by a deferred action does not
skip later defers: draining continues on its unwind continuation. If another
panic was already active it remains primary and the cleanup panic is appended
as suppressed; otherwise the first LIFO cleanup panic becomes primary. A
normal `return`, `fail`, `?`, `break`, or `continue` that encounters a cleanup
panic is replaced by that panic. Tondo 0.1 cannot catch it. `assert` evaluates
its condition and every message part from left to right; a failed assertion
concatenates ordinary and spread `Array[String]` parts without a separator. If
there are no message parts, the VM reports `assertion failed: <condition>` from
the verified source representation while the panic span supplies the location.

Host functions are reached only through verified bytecode identities. The host
receives detached `RuntimeValue` snapshots and returns another detached value;
it never receives heap handles or mutable access to VM frames. A host may keep
or mutate its snapshots after invocation without extending the reachability of
any VM object. Materializing a compound return keeps each completed child
temporarily rooted while later children allocate.

## Admission and defensive limits

Every public execution entry verifies the complete bytecode program before it
validates or pushes the selected entry frame. Invalid bytecode cannot execute a
single instruction or invoke the host. Verification, instruction steps, frame
depth, live heap objects, live heap bytes, and the initial collection threshold
all have explicit non-zero request limits.

The runtime has three distinct failure channels:

- a returned Tondo value, including an ordinary `Result`;
- a normative Tondo panic with a `P` identity; or
- a VM/toolchain error such as invalid bytecode, invalid limits, resource
  exhaustion, an unsupported host call, or an internal invariant failure.

Only the first two are program outcomes. VM/toolchain errors are never
relabelled as recoverable Tondo errors or language panics. Cooperative
cancellation is internal control state used while draining a structured child;
it is not a fourth public outcome and never becomes an implicit member of `E`.

## Required tests

The baseline suite must exercise real lowered bytecode for scalar and compound
values, direct and indirect calls, all three closure protocols, nested,
projected, generic, opaque, erased, variadic, fallible, and stateful closures,
returns, branches, loops, pattern dispatch, checked arithmetic, indexing and
slicing, collections, `assert`, `panic`, and stack traces. Heap tests retain
reachable graphs, reclaim unreachable cycles, reject stale generations, trace
and snapshot managed closure captures, and collect during construction, logical
copy, affine multi-capture moves, and invocation. Mutated HIR, MIR, and bytecode
fixtures must prove that their respective admission gates reject forged closure
identity, schema, protocol, signature, access, erasure, and effectful ordinary
calls before execution. Entry tests must accept a safe suspendible root, reject an
unsafe root, and preserve the same logical outcome contract as synchronous
`main`.

Suspension regressions execute implicit/explicit direct waits, concurrent children, concrete suspendible
closures, fallible cancellation without an injected error variant, structured
shared references, child panic, sibling cancellation/cleanup, and multiple
parked or completed managed values under a collection threshold of one.
Compile-fail fixtures cover missing initiation or scope, invalid context and
operand, non-`Send` suspension or transfer, missing `Share`, exclusive loans
across suspension, writes before join, handle escape, and unconsumed handles.
A scheduler unit test repeats dependencies, wakeups, and enqueue attempts while
proving one resumption and continued progress. Panic regressions assert cleanup
before propagation and creation-order primary selection rather than a concrete
interleaving trace.

Root-lifetime regressions force collection at every allocation while
materializing nested constants and host returns, evaluating compound operands,
building and updating collections and records, copying nested slices, and
performing elevated array arithmetic and named array sequences. A structured
pending value must survive while published and become reclaimable after
withdrawal. A retained detached host snapshot must remain valid without keeping
its former heap object alive.
The private memory adapter must additionally retain a mixed reachable cycle
through sustained pressure, reclaim independent cycles without explicit
collection calls, and reclaim the retained cycle after its root is withdrawn.
Capacity regressions must exercise recoverable and irrecoverable object and byte
limits, observe exactly one collection, and prove that failed allocation is not
counted. Replacement regressions must force collection without listing the
target as a caller root, then prove both successful publication and unchanged
target state after OOM.

`Ref` regressions must execute source-to-VM construction, prove that copying a
cell performs no second identity allocation, preserve a traced compound payload
under forced collection, reject forged move/write/exclusive bytecode paths, and
exercise equality, map replacement/lookup, and set membership with a payload
that is itself neither `Equatable` nor `Key`.

Value-copy regressions must cover every managed `Copy` shape and every enum
payload layout, prove recursive wrapper separation for a nested value, preserve
the deliberate String/`Ref` sharing exceptions, separate subsequent writes
through tuple/array, record, newtype, and map paths, and give a copied owning
cursor an independent source and position. The eager strategy retains its
allocation checks as a reference. The same black-box corpus runs eager and COW,
both normally and with collection requested at every allocation; only internal
copy counters may differ.

Iterator regressions must execute the unique static `Iterator[T]` contract and
all intrinsic cursor modes. Loaned cases cover mixed map key/value patterns,
array and map `mut`/`var` write-through, fixed-extent rejection, source-owner
conflicts, nested projections, reborrows, `break`, `continue`, `return`, normal
exhaustion, and release before later owner use. Mutated HIR, MIR, and bytecode
must reject cursor-mode drift, noncanonical positions, wrong root-loan modes,
mutable map keys, call-local or unrelated exclusive loans at the advance
boundary, and redirected element projections.

Array-length regressions must construct empty and nonempty values that share
one `Array[T]` type, observe distinct runtime lengths through source semantics,
preserve those lengths across copy, calls, and returns, and prove that the
bytecode verifier rejects a `Length` rvalue whose operand is not an array.

Array-index regressions must cover both valid endpoints in positive and
negative form, empty arrays, both `Int` extremes, positive and negative bounds
failures, value reads, writes, and borrowed places. They must prove constant
and runtime normalization share one implementation, invalid access produces
`P0001`, a simple-write RHS completes before bounds validation, and forged
non-`Int` bytecode is rejected before execution.

Array-slice regressions must cover omitted and explicit bounds, clipping in both
directions, positive and negative strides, the distinct `[::-1]` and
`[:-1:-1]` cases, empty arrays, both `Int` extremes, and `Int.min` as a step.
They must prove constant and runtime normalization share one implementation,
zero step produces `P0002`, stored and borrowed paths retain their ownership
rules, and forged non-`Int` bounds are rejected before execution.

Slice-snapshot regressions must mutate source and snapshot in both directions,
materialize directly and through a shared loan, retain `Ref` identity, copy
nested elements, exercise overlapping assignment, and prove that an exclusive
slice loan mutates only the source region. The complete public observation must
remain identical with a GC threshold of one. Bytecode mutation must also prove
that a materialized non-`Copy` slice is rejected while an affine borrowed slice
remains valid.

Array-mutation regressions must execute fixed-length changes through both a
complete `mut` owner and a sliced lender, structural replacement through a
complete `var` owner, and structural replacement of a complete nested element
without changing its outer array. Source fixtures reject a `mut` root
replacement and a `var` slice. A verified bytecode mutation then attempts a
different-length root store through a sliced `mut Array[T]`; ordinary execution
and a GC threshold of one must both reject it before publication.

Array-sequence regressions must cover dot and qualified calls, including a
shared receiver crossing a `ref` parameter, concat order, zero and empty
repetition, nested write independence, `Ref` identity, negative-count `P0011`,
length-overflow `P0005`, operand-panic precedence, and a GC threshold of one.
HIR, MIR, and bytecode mutations must independently reject a missing `Copy`
proof, changed sequence tag, wrong argument type, or non-borrowed receiver.

Loan regressions execute shared temporaries, root and projected exclusive
write-through, nested and closure-capture reborrows, statically disjoint fields,
indices, contiguous and strided slices, array-pattern prefixes and rests, and
reservations that remain active across a nested call. Dynamic single-region
loans exercise checked bounds without inventing an overlap check. Early `?`,
`break`, and `continue` paths prove explicit release, while a nested-loop
transfer proves that it cannot release an outer reservation. Mutated MIR and
bytecode reject duplicate reservation, inactive release, conflicting access,
forged overlapping collection paths, and a loan operand outside its call.

Defer regressions execute registration-time Copy snapshots, nested-scope LIFO,
final-expression, `return`, `fail`, `?`, `break`, `continue`, and panic drains,
guard retarget/disarm, guarded intrinsic Array/Map remainder cleanup, natural
exhaustion, and suppressed cleanup panics. Mutated HIR, MIR, and bytecode reject
invalid actions, duplicate guards, missing or forged transitions, repeated
registrations, post-drain reuse, incorrect drain scopes, and malformed iterator
exhaustion markers before execution.

Slice assignment materializes the complete RHS before its write validation.
The validation terminator carries aligned destination/replacement metadata,
checks normalized lengths and all destination overlap before the first store,
and produces `P0006` for a shape mismatch. The bytecode verifier rejects a
write validation whose borrowed replacement witness is absent or has the wrong
type or access form. The witness exists for every write because a plain callee
parameter can resolve to a caller slice; execution reads it only when the
normalized effective path actually ends in a slice.

Map construction carries its duplicate policy explicitly through HIR, MIR, and
bytecode. Values satisfying `Discard` use ordered last-write-wins replacement
for dynamic duplicate keys. A value that may retain a terminal obligation uses
the rejecting policy: all entry expressions are already evaluated left to
right, duplicate detection precedes map ownership transfer, and a collision
produces `P0009`.
