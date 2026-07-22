# Bootstrap VM object and execution contract

**Status:** implemented M3 baseline plus CALL-003 synchronous closure
invocation, CALL-004 effectful-environment retention with execution guards,
OWN-001 intrinsic cursor value semantics, and OWN-002 affine moves/immediate
observations, OWN-003 flow availability, and OWN-004 complete-slot
reinitialization, OWN-005 typed move paths, OWN-006 affine closure captures,
OWN-007 terminal capture obligations, and BORROW-001 call-local loan execution

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
- the identity cell used by the future `Ref[T]` surface.

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
the language contract. Copying an admitted intrinsic cursor recursively copies
its owned source (or duplicates its shared reference), preserves the current
index, and allocates an independently advancing iterator object. COW and compact
representations require differential tests against this baseline.

## Frames and roots

Execution uses an iterative Rust vector of frames; a Tondo call never recurses
through the Rust call stack. Each frame owns:

- the verified bytecode function, block, and instruction cursor;
- one state per typed slot: dead, live-uninitialized, or live with a value;
- one optional normalized reservation per function-local loan identity; and
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

`ReleaseLoan` removes a reservation when later argument evaluation takes an
early control transfer. Normal return rejects any reservation left active.
Language panic clears current-frame reservations before entering unwind, and
each propagated unwind clears the abandoned caller frame before following its
cleanup edge. Because the synchronous caller frame remains live throughout the
call, the original slot also remains a precise GC root. Host callables cannot
declare or receive borrowed parameters in the bootstrap ABI.

At every possible collection, roots are enumerated precisely from every live
value in every frame plus an explicit stack of operation-local values that have
not yet been stored. Multi-operand aggregate construction and recursive value
copy push each completed temporary until its parent object has been allocated.
Managed objects trace only their actual managed children. Iterator state and
partially moved aggregate fields participate in the same tracing walk. Moving
an affine array rest takes its contiguous elements into a new owning array,
leaves holes in the compiler-owned scrutinee, and roots both parent and moved
children across the allocation. Closure construction uses the same temporary
root stack for Copy and Move capture operands. Later suspended tasks and host
handles must add explicit root sources; they may not rely on conservative stack
scanning.

## Collector

The bootstrap collector is precise, non-moving, stop-the-world mark-and-sweep.
It has no finalizers and can reclaim unreachable cycles. Heap handles contain a
generation, so reuse of a reclaimed slot cannot make a stale handle valid.

Allocation may request a full collection when the object threshold, byte
budget, or slot budget is approached. The object being allocated and all of its
children are temporary roots for that collection. Growth of an existing object
uses the same rule. Only after a complete collection still cannot satisfy the
request does execution report VM exhaustion.

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
the pending panic crosses frames. Tondo 0.1 cannot catch it. `assert` evaluates
its condition and every message part from left to right; a failed assertion
concatenates ordinary and spread `Array[String]` parts without a separator. If
there are no message parts, the VM reports `assertion failed: <condition>` from
the verified source representation while the panic span supplies the location.

Host functions are reached only through verified bytecode identities. The host
receives detached `RuntimeValue` snapshots and returns another detached value;
it never receives heap handles or mutable access to VM frames.

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

Loan regressions execute shared temporaries, root and projected exclusive
write-through, nested and closure-capture reborrows, statically disjoint fields,
and reservations that remain active across a nested call. Early `?`, `break`,
and `continue` paths prove explicit release, while a nested-loop transfer proves
that it cannot release an outer reservation. Mutated MIR and bytecode reject
duplicate reservation, inactive release, conflicting access, and a loan operand
outside its call.

Slice assignment materializes the complete RHS before its write validation.
The validation terminator carries aligned destination/replacement metadata,
checks normalized lengths and all destination overlap before the first store,
and produces `P0006` for a shape mismatch. The bytecode verifier rejects a
slice-write validation whose borrowed replacement is absent or has the wrong
type or access form.

Map construction carries its duplicate policy explicitly through HIR, MIR, and
bytecode. Values satisfying `Discard` use ordered last-write-wins replacement
for dynamic duplicate keys. A value that may retain a terminal obligation uses
the rejecting policy: all entry expressions are already evaluated left to
right, duplicate detection precedes map ownership transfer, and a collision
produces `P0009`.
