# Bootstrap VM object and execution contract

**Status:** implemented M3 baseline plus CALL-003 synchronous closure
invocation, CALL-004 effectful-environment retention with execution guards,
OWN-001 intrinsic cursor value semantics, and OWN-002 affine moves/immediate
observations, OWN-003 flow availability, and OWN-004 complete-slot
reinitialization, OWN-005 typed move paths, OWN-006 affine closure captures,
OWN-007 terminal capture obligations, BORROW-001 call-local loan execution,
BORROW-002 last-use regions, BORROW-003 fixed versus structural mutation, and
BORROW-004 static collection-region disjunction, BORROW-005 dynamic overlap
proofs, BORROW-006 borrowed iteration, TERM-003 synchronous defer/guard
execution, TERM-004/005 structural unwind exclusivity, and GC-001 verified
trace descriptors, GC-002 complete synchronous root lifetimes, and GC-003
cycle recovery under sustained allocation pressure plus GC-004 single
pre-exhaustion collection and atomic publication, REF-001 managed identity
construction/shared content projection, and REF-002 equality and collection
keys by identity plus VALUE-001 exhaustive eager logical copies

**Language baseline:** Tondo 0.1-draft.8

This contract fixes the bootstrap object model selected by DEC-006. It is an
implementation boundary, not a source-visible memory layout or a promise for a
future native ABI.

## Values and identity

The interpreter uses an explicit `Value` enum. `Unit`, booleans, integers,
floats, bytes, characters, and function identities are immediate values. A
managed value is a generational handle into a non-moving heap slot; source code
cannot observe the slot index, generation, address, or collection schedule.
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

Closures pair a concrete bytecode callable identity with a managed environment
whose capture fields use the same optional-value move representation as other
aggregates. The environment traces every present capture and is rooted by the
closure value in a frame, another object, or the operation-local root stack.
Logical copy recursively copies the environment and every Copy capture;
immutable strings and `Ref[T]` retain their ordinary sharing rule. Snapshotting
produces the detached callable identity plus detached capture values for
tooling. Sync, unsafe, async, and async-unsafe closures share this storage
machinery; their exact effects remain in callable type metadata, not object
layout.

Compound payload fields are individually optional internally. Absence records
a logical move and is never a Tondo `none`. Bytecode verification and runtime
checks prevent an absent field from being observed as a value.

Tondo value semantics do not expose physical sharing. The bootstrap therefore
copies compound `Copy` values eagerly. Immutable strings and identity-bearing
`Ref[T]` cells may share their managed object because that sharing preserves
the language contract. One exhaustive walker recursively copies tuples, arrays,
maps, sets, closure environments, newtypes, records, all enum payload shapes,
options, results, unions, and ranges, retaining completed children as temporary
roots until the new object is published with the source descriptor. Copying an
admitted intrinsic cursor recursively copies its owned source (or duplicates
its shared reference), preserves the current index, and allocates an
independently advancing iterator object. COW and compact representations
require differential tests against this baseline.

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
cursors retain their source and expose only a verified position into it.

## Frames and roots

Execution uses an iterative Rust vector of frames; a Tondo call never recurses
through the Rust call stack. Each frame owns:

- one function identity that selects its immutable VM-derived trace descriptor
  and exact typed slot schema;
- the verified bytecode function, block, and instruction cursor;
- one state per typed slot: dead, live-uninitialized, or live with a value;
- one optional normalized reservation per function-local loan identity; and
- one ordered stack of captured deferred operations, each with a lexical scope
  and optional affine guard place; and
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
value in every active frame, captured values in its explicit cleanup entries,
and one explicit stack of operation-local values that have not yet been
published. A store, frame push, cleanup registration, or heap publication
transfers reachability to that owning container. Move, `storage_dead`, cleanup
removal, frame pop, and a temporary-scope marker withdraw it. Every
allocation-capable path restores its marker on success and VM error.

The operation-local stack covers completed constant and host-result children,
left-to-right operands, dynamic map entries, record updates, assertion parts,
recursive copies, projection and slice copies, nested array arithmetic,
variadic packing, and call preparation. The object being allocated is traced as
a pending object during the same collection, so a completed parent does not
need a second publication step. Moving an affine array rest takes its
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
temporary scope until their full managed result is published. Suspended task
frames do not exist before M7 and are therefore an explicit absent boundary,
not a fictitious root source.

Bytecode admission derives one immutable frame descriptor per function and
checks that its slot vector exactly matches the verified function. Pushing a
frame repeats that identity, count, and type check before any slot can become a
root. Live slots are still inspected through the tagged bootstrap `Value`
carrier: a function-typed slot can contain either an immediate named function
or a managed erased closure, so static type alone is not a safe root bitmap.
The descriptor instead proves which schema is being interpreted. A future
suspended frame retains the same function identity and slot representation, so
it selects the same descriptor without copying its schema. M7 must register
that new container as an explicit root source before suspension can become
constructible, but it does not need another frame layout.

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

`Pointer[T]`, `Join[T,E]`, `Command`, and `Pipeline` currently have no
constructible managed bootstrap representation and therefore admit no heap
object descriptor. M7 must extend the sealed catalog before any new runtime
object shape can be allocated; an ad-hoc object-side tracing method is not an
extension point.

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

## Control flow, calls, and panic

The VM executes verified branches, tag dispatch, loops, iterators, calls,
returns, and cleanup edges directly. Checked operations either produce a value
for their normal successor or begin a language panic on their unwind successor.

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

This call path admits only signatures with neither `async` nor `unsafe`. The
bytecode verifier rejects an effectful ordinary call, and the public execution
entry rejects selecting an async or unsafe callable body as the root frame.
Effectful closures can therefore be constructed, copied, traced, snapshotted,
erased to the identical effect-preserving function type, and discarded without
activating an unfinished async runtime or bypassing an unsafe context proof.

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
relabelled as recoverable Tondo errors or language panics.

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
calls before execution. Entry tests must also reject async and unsafe callable
bodies while their runtime contexts remain unimplemented.

Root-lifetime regressions force collection at every allocation while
materializing nested constants and host returns, evaluating compound operands,
building and updating collections and records, copying nested slices, and
performing elevated array arithmetic. A structured pending value must survive
while published and become reclaimable after withdrawal. A retained detached
host snapshot must remain valid without keeping its former heap object alive.
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

Eager-copy regressions must cover every managed `Copy` shape and every enum
payload layout, prove recursive allocation for a nested value, preserve the
deliberate String/`Ref` sharing exceptions, separate subsequent writes through
tuple/array, record, newtype, and map paths, and give a copied owning cursor an
independent source and position.

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
