# Typed HIR to MIR contract

**Status:** M3 typed CFG plus M4 uniform function values, four effect-preserving
closure forms, executable synchronous-safe closure calls, static-trait and
opaque-result lowering, OWN-001 intrinsic cursor state, OWN-002 affine
transfers, OWN-003 flow availability, OWN-004 complete `var` reinitialization,
OWN-005 typed move paths and uniform match ownership modes, OWN-006 contextual
Copy/Move closure captures, OWN-007 all-exit terminal capture obligations, and
BORROW-001 call-local loans plus BORROW-002 inferred pattern regions with
explicit reservation/release, BORROW-003 permission-preserving reborrows,
BORROW-004 static collection disjunction, BORROW-005 runtime overlap proofs,
BORROW-006 loaned-iteration boundary verification, TERM-001/TERM-002 terminal
classification and normal-path ownership, and TERM-003 explicit defer/guard
cleanup, TERM-004 closed abnormal fallback lowering, and TERM-005 exact
explicit/fallback exclusion, REF-001 identity aggregates/shared projections,
REF-002 identity equality/key operands, ARRAY-001 runtime array length,
ARRAY-002 positive/negative checked indexing, ARRAY-003 slicing, ARRAY-004
logical slice snapshots, ARRAY-005 fixed versus structural array mutation,
ARRAY-006 closed lifted arithmetic, ARRAY-007 named concatenation/repetition,
and ITER-001/002 static user iterators plus all four intrinsic iteration forms
implemented, plus TEXT-002 Unicode-scalar String length, indexing, and slicing,
and TEXT-003 static Display calls plus ordered interpolation, plus
VARIADIC-001/002 homogeneous final packs and whole-array spread, plus the
historical explicit-await ASYNC-001..004 prototype, SCOPE-001, SPAWN-001,
JOIN-001, SEND-001, SHARE-001, and MAIN-ASYNC-001; the 1.67 implicit-await
migration remains pending

This document fixes the internal contract required by M3, M5, and M7. It does
not define observable source-language behavior; `TONDO_LANGUAGE_SPEC.md`
remains normative.

## Admission boundary

`verify_typed_hir` is the only admission path from semantic analysis to MIR.
It runs for every complete, error-free expression check, including ordinary
`tondo check`, so an invariant defect is discovered before a backend exists.
`lower_to_mir` then constructs the graph and runs `verify_mir` before returning
it to a backend. Failures are internal `HirInvariantError` or
`MirInvariantError` values, never source diagnostics.

An admitted program guarantees:

- expression, flow, and reachable-break arenas have identical lengths;
- every reachable `TypeId` has a canonical representation and therefore
  contains neither recovery nor unresolved inference;
- every expression and pattern child is a valid earlier arena entry, making
  both arenas acyclic and topological;
- recovery expressions, recovery patterns, unresolved call arguments, and
  unresolved loop transfers are absent;
- every local, symbol, member, callable, nominal constructor, field, and
  variant reference exists in the same `ResolvedProgram` and has a compatible
  declaration kind;
- every expression has exactly one `Value` or `Place` category consistent with
  its operation and base projection;
- constants have a checked initializer and normalized compile-time value of
  the same type;
- callable IDs are unique and deterministically ordered, and every source body
  has one checked root;
- every concrete closure has one effect-specific generated type, matching exact
  signature, independently derived protocol row, independent body root, one
  construction expression, and an exact owned capture table;
- every indirect call has one exact effect-bearing signature, selected call
  protocol, and source access form accepted by typed HIR; suspendible calls
  lower to an implicit `Await` unless `Spawn` is explicit;
- every prelude trait operand has complete canonical arguments and the exact
  `Display.display` or `Iterator.next` function type;
- every ordinary named-function operand is either intrinsically non-generic or
  carries one complete specialization whose exact substituted signature is its
  operand type;
- every iterator loop records either a valid intrinsic source plus its exact
  `cursor[own,C]`/`cursor[ref,C]`/`cursor[mut,C]` state type, or one exact
  `Iterator[T]`
  contract whose element matches its binding pattern;
- every opaque result has one verified declaration contract and finite witness,
  and every representation seal relates that exact witness to its opaque family;
- loop IDs are unique, transfers and break summaries target existing loops,
  and direct transfers are marked as diverging; and
- member occurrences, annotations, local types, pattern fields, aggregate
  fields, and generic arguments reference valid canonical entities.

Partial semantic snapshots intentionally need not satisfy these properties.
They remain queryable but can never be lowered or executed.

## Responsibility split

| Phase | Facts proved or represented |
|---|---|
| Resolution | Namespaces, declaration/member/local identity, visibility, and lexical binding |
| Typed HIR | Static types, contextual conversions, opaque contracts and witnesses, effect-exact concrete closure signatures, capture sets and call protocols, implicit suspension or explicit handle consumption, safe `spawn` initiation, task-scope and `Join` provenance, value/place category, pattern coverage, source evaluation order, and source-level control targets |
| MIR construction (M3/M4/M5) | Typed locals and temporaries, explicit CFG, places, synchronous calls plus implicit-suspension lowering, effect-preserving closure bodies with a hidden environment, contextual Copy/Move closure-environment construction, branch targets, normal/abnormal edge shape, and spans |
| Ownership MIR (M5) | Contextual `Copy` versus `Move`, immediate non-escaping observations, whole-owner source availability, typed internal move paths, uniform `match` copy/observe/consume lowering, call-local `ref`/`mut`/`var` loans, inferred last-use pattern regions, static and runtime-checked collection regions, canonical borrowed-iterator boundaries, explicit scope-nested defer registrations with affine guard transitions, and closed intrinsic fallback actions |
| Async MIR (M7) | `Await`, `Spawn`, task-scope entry/drain, normal and tagged abnormal edges, exact suspension-live frame state, and `Send`/loan checks across suspension |
| Bytecode/backend | Layout and executable instructions only; no source semantic inference |

No later phase performs fallback name lookup, repeats overload selection,
chooses a contextual conversion, reconstructs a pattern from syntax, or
changes source evaluation order.

## CFG shape

A `MirProgram` contains deterministic functions. Each function owns:

- a typed local table containing parameters, user locals, the return place, and
  compiler temporaries;
- basic blocks in stable allocation order;
- statements that complete within their block; and
- exactly one terminator per block.

Places begin at one local and carry typed projections. Dynamic indices, slice
bounds, receivers, keys, and other effectful operands are evaluated into
temporaries before a place uses them. This preserves the HIR rule that an
assignment resolves every destination once before evaluating its RHS.

Operands distinguish constants, copy reads, move reads, immediate borrows, and
call-local loan identities. Places may additionally name the shared region loan
that authorizes their projected source access.
OWN-002 chooses `Copy` or `Move` from the HIR capability graph under the exact
generic bounds of each body. A `T: Copy` body copies; an unbounded or merely
`T: Discard` body moves. The decision is cached per body and type, retained
through monomorphization, and rederived by the MIR verifier. OWN-003 adds the
flow fact that a moved place is unavailable afterwards. A backend never decides
between copy and move from runtime representation alone.

OWN-003 proves source-local availability in HIR. OWN-005 keeps that source model
deliberately small: Tondo does not expose a persistent partially moved owner.
Standalone transfer of a non-`Copy` field, tuple slot, array element, slice, or
borrowed location is rejected with `E1406`. Full destructuring first consumes
the complete owner and only then transfers selected components inside the
compiler-owned operation. The intrinsic `.value` projection of a newtype is
the one complete-owner projection and consumes its owning binding, ordinary
value parameter, or movable temporary.

`Ref(value)` lowers to one `MirAggregateKind::Ref` with exactly one operand and
the exact `Ref[T]` result type. The operand retains HIR's contextual access:
`Copy` for a `Copy` target and `Move` for a merely `Discard` target.
`Ref[T].value` lowers to `MirProjectionKind::RefValue`; it participates in
typed paths so nested reads and loan overlap keep one canonical origin, but it
is never a complete-owner projection. A `Copy` read is valid only when
`T: Copy`, a call-local shared loan may observe any admitted `T`, and no
`Move`, value-producing destination (`Assign`, `Invoke` or `IteratorNext`),
write validation, `mut` loan, or `var` loan may contain this projection.

MIR independently represents availability as a live-storage bit plus a
canonical set of unavailable typed move paths. Paths cover closure captures,
record and tuple fields, newtype values, enum/option/result/union payloads,
array-pattern indices and rests, dynamic indices, and slices. A read or move is
valid only when its path overlaps no unavailable ancestor, descendant, or same
path. Statically distinct fields, tuple slots, variant payloads, and disjoint
array-pattern regions may move independently; dynamic indices and slices are
conservatively overlapping unless their disjunction is proved elsewhere.

A move inserts its path and subsumes unavailable descendants. A complete write
clears every unavailable path. A projected write requires an available parent
and restores exactly its subtree, which is needed for compiler-internal atomic
transformations but does not grant source-level partial reinitialization. At a
CFG join the unavailable sets are unioned and storage liveness is intersected,
including loop backedges. `Invoke` and `IteratorNext` define their destination
only on the successful edge. OWN-004 therefore still needs no recovery
instruction: the RHS completes before a direct `var` write creates the new root
definition, while an unwind edge observes no such write.

Every HIR `match` records one `Copy`, `Observe`, or `Consume` mode and the HIR
verifier rederives it. Tests, tags, shape checks, and guards borrow from one
stable place or compiler-owned temporary. Copy bindings needed by a guard are
materialized before it; affine bindings move only after the guard succeeds in
the selected arm. A false guard therefore cannot consume a payload needed by a
later arm. Pattern `ref` aliases exist only while constructing MIR and lower to
their real projected place. The resulting place retains only a `source_loan`
identity, not a duplicate source local or first-class reference value.

Branches use block IDs, not nested expression nodes. `Never`, `return`, `fail`,
`break`, `continue`, propagation, and panic paths end in terminators without an
invented normal successor. A block is never left unterminated, including
syntactically unreachable blocks retained for spans or diagnostics.

The M3 lowering covers every expression admitted by complete typed HIR,
including short-circuit operators, all three loop forms, exhaustive patterns
and guards, assignment, construction and update, collections, indexing,
slicing, numeric conversions, calls, and both `Option` and `Result`
propagation. Recovery nodes cannot cross the HIR admission boundary and
therefore have no executable MIR interpretation.

Checked numeric conversion produces the exact
`Result[target, NumericConversionError]` type. The three intrinsic error values
lower through a dedicated zero-payload aggregate and dedicated MIR tags, so
pattern exhaustiveness and branching do not depend on a fabricated nominal
declaration. MIR verification rederives the closed conversion class, result
shape, intrinsic error type, and valid tag before bytecode lowering.

An array literal lowers to `MirAggregateKind::Array` with one operand per
runtime element and an `Array[T]` result type that contains no length. Array
pattern shape checks lower through `MirRvalueKind::Length`; the MIR verifier
accepts that rvalue only from `Array[T]` and requires an `Int` result. Calls,
returns, and Copy/Move selection forward the complete array value, so no phase
may reconstruct its length from the static type.

A set literal similarly lowers to `MirAggregateKind::Set` without performing
deduplication in the compiler backend; ordered normalization is an observable
runtime operation because equal dynamic operands may only be compared after
their left-to-right evaluation. A range lowers to one `MirRvalueKind::Range`
carrying its inclusive/exclusive kind and two already typed endpoints. MIR
verification requires identical discrete integer or `Char` endpoint types and
the exact corresponding `Range[T]` result.

An ARRAY-007 HIR node evaluates its receiver through `lower_borrowed_value`,
then its argument through ordinary value lowering, and emits one checked
`MirOperationKind::ArraySequence` `Invoke`. The operation records `Concat` or
`Repeat`, returns the same `Array[T]` type as its receiver, and always owns an
unwind edge because length overflow or an invalid repeat count can panic. The
MIR verifier requires a `Borrow` receiver, rederives `T: Copy`, and requires the
argument to be the same array type for `Concat` or exact `Int` for `Repeat`.
Changing the operation kind, receiver access, argument, result, or capability
therefore invalidates MIR before bytecode lowering.

MAP-002 lowers `Map.remove` after reserving an exact `var` region for its stable
receiver place. `MirRvalueKind::MapRemove` carries that sourced `Map[K, V]`
place and the already evaluated `K` operand and produces exactly `Option[V]`.
It is non-panicking: absence is represented by `none`, so the operation remains
a `Store` rvalue rather than an `Invoke`. The verifier rederives the map
arguments, result payload, exact region origin, and `var` mode. Its write-access
event is permitted only through that source region and conflicts with every
other overlapping active loan.

A concrete closure expression lowers to one aggregate with its `HirClosureId`
and captures in the exact HIR table order. Each operand is an unprojected Copy or
Move of the MIR local for that exact outer `LocalId`, selected under the exact
inherited generic assumptions. The aggregate result retains the
effect-specific generated closure type. Its independently rooted body becomes a
`MirFunctionId::Closure` function. Slot zero is a hidden environment parameter;
capture references are typed projections from that slot, and explicit source
parameters follow it in their original order. A capture projection may itself
move, and availability then rejects every overlapping later use on the same CFG
route. Construction remains separate from body execution. The body and exact
function signature may represent sync, unsafe, suspendible, or
unsafe-suspendible source effects. A suspendible body is reached by the
implicit-await call operation; only `spawn` preserves it as a pending handle.

An indirect closure call carries the exact protocol selected by HIR. `Call` and
`CallMut` read a place through a shallow, non-escaping `Borrow` operand so body
updates observe the original environment; `CallMut` additionally requires the
source place to be writable. `CallOnce` uses the ordinary Copy or Move operand
selected by source access, including a non-`Copy` generic or opaque callable.
Its protocol row is valid only when every non-`Discard` capture path is moved on
every reachable normal `Return`. The verifier computes that fact as a CFG
must-analysis: predecessor states intersect, a complete capture or chained
newtype-value move inserts the capture, and any later write removes it. Cleanup,
panic, and unreachable blocks do not create normal return obligations.

`Borrow` remains a shallow operand for one immediate observation only:
equality, membership, length, discriminant branches, the collection base of
index/slice, an indirect `Call`/`CallMut` callee, intrinsic ref-cursor
construction, and the replacement whose length is checked before a slice
write. It may never be stored, returned, inserted into an aggregate, passed as
a value argument, or used by an unrelated operation. It is not the
representation of a non-value call argument.

BORROW-001 and BORROW-002 give each function one dense loan table. Every entry
has a `CallLocal` or `Region` kind, one non-value mode, and one fully evaluated
place. A call-local entry is reserved after its argument place has been
resolved; `Loan(id)` is the only valid operand for the matching non-value call
argument, and the call consumes it as one terminator event. Explicit releases
close reservations abandoned during evaluation of a later argument.

A pattern `ref` binding instead creates one shared `Region` entry. The alias
place carries that region as `source_loan`; a nested pattern region or
call-local reborrow points to the immediately enclosing region while retaining
the same owner path. Once the original CFG is complete, MIR runs backward
liveness over normal successors. Anchored place uses and loan operands retain
their complete region dependency chain. A last statement use receives an
immediate `ReleaseLoan`; branch-specific last uses split only the affected
normal edge into a release bridge. Releasing an abandoned call-local reborrow
also counts as a use of its source chain. Releases use reverse loan order so a
call-local child or nested region always closes before its sources. No source
lifetime or reference-shaped local is introduced.

The verifier propagates the exact active-loan set over the CFG and requires all
predecessors of a join to agree. It independently proves that region metadata
is shared `ref`, source chains are earlier and acyclic, anchored places remain
inside the reserved source path, and every anchored access has its complete
source chain active. A source region cannot close while any transitive child
reservation remains active. Shared reservations may overlap only other shared
reservations; exclusive reservations reject every overlapping access or
reservation. Fixed record fields and tuple slots may be disjoint. Reborrowing
permits `ref` from any borrowed source, `mut` from `mut`/`var`, and `var` from
`var` or from a complete strict projection of `mut`. The verifier rederives that
the latter ends in a field, tuple/newtype/variant payload, closure capture,
array-pattern element, or existing array element; a root, slice, array rest,
potential map entry, or opaque projection cannot upgrade `mut` to structural
access. Moves out of borrowed parameters and writes through `ref` are invalid.
A `Region` can never be consumed as a call operand; a `CallLocal` may be
consumed at most once.

Releases remain explicit on `return`, `fail`, `?`, `break`, `continue`, and
ordinary last-use edges. Panic edges enter unwind with an empty loan set because
runtime unwinding invalidates every reservation in the abandoned caller frame;
cleanup blocks therefore contain no loan manipulation. BORROW-004 admits index,
slice, array-pattern element, and array-pattern rest loan projections. MIR
recovers non-negative constants from single-definition temporary locals and
rederives the same static region relation used by HIR. Statically disjoint paths
may coexist without an overlap check; inevitable overlap remains invalid.
BORROW-005 runs a post-liveness dataflow pass over the exact active-loan set and
attaches canonical `against` loan IDs only to relations whose answer remains
runtime-dependent. It does this for a new `ValidateLoan` terminator, indexed and
sliced `Invoke` operations, and each place carried by `ValidatePlaces`.
A loan operand has no valid use outside its consuming call and cannot reach a
branch condition, rvalue, aggregate, host boundary, return, or storage.

User `Iterator.next` calls use the same explicit call-local `mut` loan for
their state receiver. A `cursor[ref,C]` is restricted to stable `Array`, `Map`,
or `Set` places; `cursor[mut,C]` is restricted to stable writable `Array` or
`Map` places. Its source is one shared or exclusive `Region` loan held for the
whole loop; `IteratorNext` writes only a checked integer position, and
`IteratorElement { index }` projects the current item directly from that
borrowed source. Dynamic indices that select a nested source are copied into
single-assignment temporaries before the region is reserved, so the cursor's
identity cannot change during the loop. The terminator records the source place
explicitly. The verifier ties cursor construction, source loan, position
destination, and every element projection to one canonical origin, so a forged
cursor cannot redirect a loan or turn loaned iteration into an element copy.
Per-pattern `ref`, `mut`, or `var` children are reserved inside the body and
released on every backedge or exit. Exclusive `Map` children must project tuple
field 1, preserving keys and insertion order; `mut` array replacement retains
length while `var` may replace the selected element.

`IteratorNext` is a verified loan boundary: its exact incoming set may contain
shared `Region` loans plus the cursor's complete canonical source chain. Any
exclusive region must belong to that chain; call-local loans and unrelated
exclusive regions are rejected. This admits a nested cursor over a borrowed
element without allowing an independent mutation to cross the advance. The
unwind edge starts with an empty set. `Await` reuses this active-set proof:
call-local loans and every live exclusive region are rejected, surviving
ordinary owners must satisfy `Send`, and only a `Join` owned by the current
task scope receives its sealed exception. `Spawn` admits its explicit
structured shared loans and retains them until the corresponding handle is
consumed or torn down. The ordinary MIR call operation may be synchronous or
lower to an implicit `Await` and retains an explicit `unsafe_call` bit. That bit must agree exactly with the
selected callable signature; it is true only after HIR proved an active unsafe
region. `Await` and `Spawn` carry the same exact call operation for suspendible work,
including the independently verified unsafe bit. Raw Pointer operations have
six distinct host-operation identities and complete receiver, argument, and
result type checks; they cannot be forged from a safe host call.

Checked operations use `Invoke`; indexed and sliced reads therefore cannot
bypass their bounds/unwind edge. Assignment first resolves all destination
places, then materializes its complete RHS, then validates overlap, bounds, and
slice replacement lengths before performing any write. Each validated place
carries an aligned `against` list for active runtime-dependent conflicts.
An array index remains one `Int` operand evaluated exactly once; MIR does not
pre-normalize a negative value because the current array length is required.
Both the checked read operation and projected place use the same operand local,
and every failure follows the function's ordinary language-panic unwind edge.
An array slice similarly retains three optional `Int` operand locals in
start/end/step order. Omission remains structural metadata rather than a
manufactured sentinel, and MIR never normalizes, clips, or advances the
indices. The checked operation and every projected place reuse those exact
locals; a zero step and every later loan/write validation follow the same
language-panic unwind edge.
`MirOperationKind::Slice` over an array always materializes an owning
`Array[T]` and the MIR verifier therefore rederives `Array[T]: Copy` under the
exact function generic assumptions. A slice that remains inside `ref`/`mut`
loan metadata is only a place projection and does not require `T: Copy`; it
never passes through the materializing operation.
String index and slice reads use the same checked operations and operand order.
The verifier instead requires `String + Int -> Char` for an index and
`String + bounds -> String` for a slice. Neither form can enter place
projections, runtime-overlap metadata, or write validation, preserving text
immutability independently of HIR construction. `Length` admits only `Array`
or `String` and always produces `Int`; String execution counts Unicode scalars.
Display conversion remains an ordinary statically selected call in MIR. Its
receiver is one call-local shared loan, including when the source value is a
temporary. Once every hole has produced `String`, one
`MirRvalueKind::Interpolate` retains the decoded segments and those result
operands in exact source order. The MIR verifier rejects a non-String result,
a non-String completing operand, or any segment/value arity mismatch.
`MirOperationKind::ArraySequence` is likewise always checked. Its shared
receiver is the sole permitted immediate `Borrow`; its value argument cannot
contain a borrow or loan. The explicit operand order fixes
receiver-before-argument evaluation, while runtime preflight owns `P0005` and
`P0011`.
Compound assignment
uses an access validation before reading its previous value and validates the
fully computed replacement again before storing it. Every write validation
carries a borrowed replacement witness of the destination type. This is needed
even for a syntactically unprojected borrowed parameter because its effective
lender path may end in a slice; the VM consults the witness only in that case.
Write validation of an unprojected destination does not read its previous
value; every projected destination still reads an available root while
resolving its path.
HIR admission has already rejected arbitrary root replacement through
`mut Array[T]` and every `var` region loan. MIR retains the exact `mut`/`var`
parameter and loan modes: in-place array results therefore return through a
`mut` carrier, structural replacement through `var`, and a sliced lender can
only follow the fixed-extent path. Runtime length remains deliberately absent
from MIR types and is checked at publication.

Before reserving any index or slice loan, MIR emits `ValidateLoan` with a normal
successor and the current cleanup successor. Its success block must immediately
reserve that same loan. The terminator resolves every already-materialized
index/bound exactly once for bounds and zero-step failure; its `against` list is
empty for statically disjoint active loans and contains exactly the dynamic
conflicts that require a runtime comparison. An earlier argument reservation is
cleared by unwind if a later validation fails, so no partial loan reaches the
callee. Indexed and sliced reads carry the same proof atomically in their
`Invoke`, while `ValidatePlaces` hands a pending proof to the immediately
following read or write. The verifier rederives every expected ID from CFG
liveness and static-region facts, rejects missing or extra IDs, requires pending
loan proofs to be consumed by the matching reservation, requires access proofs
to be consumed before another terminator, and forbids changing materialized
index/bound locals while either proof is pending. Static callees remain callable operands
instead of being erased into ordinary
temporaries, preserving the selected declaration, receiver mode, generic
specialization, and variadic argument association. Source-trait calls retain
their specialized trait member;
`Display.display` and `Iterator.next` use a dedicated prelude operand with their
complete type arguments. These operands carry no vtable or runtime witness and
are resolved to direct implementation callables during monomorphization.

Storing or passing a function value uses the ordinary typed local, constant, or
aggregate path. A later call through that place is therefore genuinely indirect
in MIR. Its source type may be concrete, generic, or opaque, but HIR records one
exact structural function signature and call protocol. Arguments are indexed
positionally and preserve modes and variadic association; no parameter label
survives in the function type. The MIR verifier checks the same exact call
contract whether the callee is a static function operand or a value read from a
place.

For an individual VARIADIC-001 element, lowering retains one
`VariadicElement` association and the element's ordinary Copy or Move operand.
It does not synthesize a tuple, erase `T`, or reorder evaluation. A call with no
such operands still retains the variadic function shape, allowing the runtime
to construct the body-visible empty `Array[T]`. The MIR verifier rederives the
unique final by-value element type from the exact call signature and rejects a
forged fixed, receiver, mode, or element-type association.

VARIADIC-002 retains one unique final `VariadicSpread` association whose
operand is exactly `Array[T]` by value. Ordinary contextual lowering selects
Copy when the closed array is `Copy` and Move otherwise; MIR does not expand
the array into synthetic element operands. The verifier independently rejects
repetition, non-final position, wrong array element, borrowed access, and a
forged Copy of an affine array.

An opaque success exit remains an explicit coercion rvalue whose kind is
`Assignability::Opaque`. MIR preserves both operand and destination types, so a
later phase never needs to rediscover the hidden representation. The coercion
has no runtime transformation: its purpose is to keep the declaration-owned
seal auditable across the typed CFG. For a fallible function the ordinary
`Result` construction and propagation remain outside that success seal, so the
visible error channel is unchanged.

Intrinsic `for` sources use an iterator-state rvalue whose operand is the
collection `C` and whose result is the distinct concrete
`cursor[own,C]`/`cursor[ref,C]`/`cursor[mut,C]` local consumed by
`IteratorNext`. The verifier
rejects both a cursor disguised as its collection and a cursor whose mode or
collection differs from typed HIR. A user `Iterator[T]` source is evaluated once
into a state local; each header invokes the typed `Iterator.next` operand with
an explicit call-local `mut` loan, observes the returned `T?` discriminant,
projects the dominated
`Option` payload, and then binds the irrefutable loop pattern. The MIR shape
therefore exposes every evaluation and edge without treating a user iterator
as a VM intrinsic.

All three intrinsic cursor modes are admitted. Loaned cursors retain their
runtime-selected source region and loop-spanning lifetime explicitly; neither
shared nor exclusive iteration is ever approximated with a collection copy.

Map construction is an `Invoke` carrying the HIR-selected duplicate policy, so
`P0009` has an ordinary unwind edge and last-write-wins is never an implicit VM
choice.

An `assert` operation also carries the checked condition's nonempty source
representation. The MIR verifier rejects its loss before bytecode lowering,
while the condition and message operands remain in ordinary evaluation order.

## Cleanup and suspension capacity

Every call or checked operation that may panic has an explicit unwind target.
Normal scope exits and transfers route through cleanup blocks, even when the
M3 cleanup chain is empty and collapses to a direct edge. Cleanup blocks are
marked so verification can reject an edge that re-enters ordinary execution.
Loan release is already explicit on normal last-use edges, control transfers,
and early function exits; an unwind edge closes the abandoned frame's
reservations as part of panic propagation.

TERM-003 and TERM-004 populate those blocks with six explicit forms:

- `RegisterDefer { scope, action, guard }` stores one already checked
  infallible `Unit` invocation. A synchronous action uses a sync call signature;
  `defer await` retains the suspendible call signature and is admitted through the
  `DeferredAsync` operation context. Copy operands are snapshots; an optional
  guard names its unique complete affine owner slot, including one environment
  capture slot while lowering a closure body. No suspendible block or fallible cleanup
  operation can reach this statement form.
- `RegisterFallback { scope, owner }` arms the sealed structural unwind action
  for a non-absent terminal owner. Owning parameters register at entry,
  closure environments register each terminal capture independently, and
  terminal store, call-result, and iterator-value edges register immediately
  after materialization.
- `RetargetCleanup { from, to }` follows an immediately preceding whole-value move
  of either cleanup kind to an equal-typed complete owner slot. The sole
  non-slot target is the intrinsic owning cursor's exact `IteratorSource`
  projection.
- `DisarmCleanup(place)` removes an explicit guard or transfers/discharges an
  abnormal fallback only after an immediate confirmed handoff, terminal
  operation, or proved natural exhaustion.
- `DrainDefers { scopes, target, unwind }` executes registrations belonging to
  the exact abandoned scopes in global LIFO order, but selects only explicit
  entries. It branches according to whether cleanup completed or panicked.
  Every drained guard is consumed on both successors and remains unavailable
  until a complete owner write reinitializes it.
- `DrainUnwind { target }` pops the unified ledger in exact reverse
  registration order, executing both explicit entries and armed structural
  fallbacks before reaching the distinguished panic-resume block.

Normal completion and `return`, `fail`, `?`, `break`, and `continue` route
through the exact inner-to-outer explicit drain set. Remaining fallback markers
are removed without execution only at a normal return, after TERM-002 has
already proved the visible consumption or handoff; an explicit registration
may never be abandoned there. Every checked-operation panic edge targets one
shared `DrainUnwind` block, which then reaches `ResumePanic`. The ledger is
independent of loan state: reservations are released before a normal drain or
invalidated when the frame begins unwinding.

An owning intrinsic `IteratorNext` additionally records an optional
`exhaustion_guard` naming the exact `IteratorSource` projection in its state.
It is present when the contextual collection status is `Present` or
`Potential`, absent when that status is `Absent`, and affects only the
`exhausted` successor. Generic `Potential` MIR retains the conservative marker;
closed bytecode specialization removes it when the concrete collection is
nonterminal. A ref cursor and a user `Iterator.next` call never carry this
marker.

TERM-001 supplies MIR with the verified closed registry and structural status.
TERM-002 rejects unconsumed normal-path owners. TERM-003 materializes explicit
defer registrations and guards without inventing a destructor. TERM-004
materializes the other side of that same ledger: closed structural fallbacks,
coverage at every ownership-materialization edge, and one abnormal LIFO drain.
TERM-005 closes their exclusion proof. For a `Present` or `Potential` terminal
guard, `RegisterDefer` must replace exactly one fallback contained by that
complete guard before it can become active; replacing zero or more than one is
invalid. A later `RegisterFallback` may not overlap that explicit guard.
Because retarget and disarm transitions address the sole active entry, no
move, aggregate, call, or iterator edge can duplicate the obligation. Capture
failure occurs before the replacement and therefore leaves the original
fallback armed.

`Await` is a suspension terminator whose awaitable is either one complete
suspendible operation or one `Join`/`Waiter` operand. Direct suspendible calls
lower to this terminator implicitly; an explicit `await` selects the same shape.
It writes the logical result only on `target`;
panic and cooperative cancellation use the explicit `unwind` edge while
retaining their distinct tagged runtime state. Values live across the
terminator remain ordinary typed locals in a frame that the executor may park.
An exclusive loan may not be live there; the BORROW-006 boundary check is
reused for the exact active set, and all surviving values must satisfy the
required `Send` contract before bytecode generation.

A suspendible deferred call does not use an `Await` terminator at registration. Its
operands are captured by `RegisterDefer`; the cleanup drain later transfers the
same suspendible operation to the executor, preserving the surrounding unwind edge
and LIFO position.

`EnterTaskScope` pushes one lexical identity. `Spawn` names the active
innermost identity, transfers one suspendible operation, and writes the exact
`Join[T, E]` only on its normal edge. `DrainScopes` lists the exact
inner-to-outer task-scope suffix and the lexical defer scopes that follow it.
Every return, propagation, loop transfer, panic, and cancellation path therefore
has an explicit order: request child cancellation, suspend until child cleanup
finishes, consume remaining join fallbacks, and only then drain the affected
defers. The verifier rejects scope-stack disagreement at CFG joins, spawning
into an inactive scope, or returning with an active task scope.

## MIR verification layers

The structural verifier introduced in M3 proves at minimum:

- every block has one valid terminator and every successor exists;
- local, field, variant, function, and constant indices are in range;
- every operand and destination agrees with the declared local/type table;
- every use is dominated by an available typed path, every root or projected
  move consumes that path exactly once on every CFG route, and no local is
  accessed outside its declared storage lifetime;
- place projections are legal for their base type;
- call arity, modes, argument types, and outcome agree with the selected
  callable, and every indirect call repeats the exact HIR signature/protocol
  selection for concrete, generic, and opaque callees;
- a direct ordinary call either has a clear effect bit or lowers to `Await`; a
  `suspends` callable is never executed synchronously by retaining its body;
- every static function operand has complete specialization arity and its exact
  substituted type, while an indirect callee has that same concrete structural
  function type;
- prelude trait operands have their complete arity and exact closed signature,
  including the single receiver parameter expected by a call;
- an opaque coercion is used only from the declaration's exact concrete witness
  to the matching opaque family, while no other coercion kind may forge that
  relation;
- aggregate, conversion, iterator, index, slice, range, membership, and tag
  operations have the exact instantiated input and result types;
- a `Ref` aggregate has one payload matching its intrinsic target, every
  `RefValue` projection has that exact target type, and a projected path is
  never moved, written, or reserved by an exclusive loan;
- a closure aggregate names existing HIR metadata, has the exact generated
  result and capture layout, and contextually copies or moves each capture from
  the corresponding unprojected outer source binding rather than a merely
  type-compatible value;
- every closure has exactly one body function with its generated environment as
  hidden parameter zero, exact explicit parameters, capture projections, and
  outcome, while no ordinary function may forge that shape; all four effect
  signatures are retained unchanged;
- each closure protocol row is rederived from writes, exclusive uses, and moves
  of its typed environment paths, so a moved capture cannot advertise `Call` or
  `CallMut`; `CallOnce` additionally requires every non-`Discard` capture to be
  completely transferred on every reachable normal return;
- `Borrow` appears only in an enumerated immediate observation, as an indirect
  `Call`/`CallMut` callee, or as the exact source of `cursor[ref,C]` or
  `cursor[mut,C]`; it never
  escapes into storage, call arguments, aggregates, returns, or arbitrary
  rvalues, and `CallOnce` never uses it;
- every non-value argument consumes one matching call-local `Loan` identity
  after an explicit reservation; every pattern region is a shared reservation
  whose anchored accesses occur while its acyclic source chain is active;
  active sets agree at CFG joins, incompatible fixed regions cannot overlap,
  abandoned paths release their reservations, and no loan escapes its extent;
- every loaned iterator has one stable source-region origin, one canonical
  position producer, and only its mode-exact root region loan crossing its
  advance boundary;
- every explicit or fallback registration is type-valid, no dynamic
  registration is repeated before drain/disarm, cleanup places are pairwise
  compatible, every terminal explicit guard replaces exactly one fallback,
  neither kind can be rearmed across the other, and every guarded move has one
  immediate exact retarget/disarm transition;
- every owning terminal entry parameter/capture and every terminal store,
  successful invocation result, and iterator-value edge has its required
  fallback or retarget, including conservative `Potential` generic MIR;
- every normal drain names exactly the explicit scopes abandoned by its edge,
  while the single abnormal drain targets `ResumePanic`, preserves unified
  LIFO order, and leaves no registration at panic resume; a normal return may
  discard fallback markers but never an explicit entry;
- an intrinsic own iterator carries the exact source exhaustion guard if and
  only if its contextual collection status is non-absent, and only its
  exhausted successor removes that guard;
- equality, collection membership, and map lookup satisfy the `Equatable`,
  `Key`, or `Copy` requirement recorded and independently verified in HIR;
- a variant, union, option, or result payload is read only on an edge dominated
  by the corresponding discriminant test, and writes invalidate refinements;
- cleanup edges enter cleanup blocks and cleanup blocks cannot return to an
  abandoned normal path; and
- source spans remain attached to locals and every executable operation, and
  stay within the function's source file.

Definite initialization, typed move-path availability, and storage lifetime are
forward dataflow properties, not assumptions made by bytecode generation.
Parameters are initialized at entry, edge-specific results are initialized only
on their successful edge, and the return place must be initialized on every
`Return`. Payload refinement is a separate forward analysis so initialization
alone cannot authorize an invalid projection.

The suspension verifier additionally proves operation effect and call protocol,
awaitable/result agreement, `Join` move access, active innermost spawn scope,
scope-stack balance, exact normal/unwind destinations, suspension liveness,
`Send`/`Share` requirements, and the exclusion of exclusive loans. It reuses
the existing BORROW-006 loan-boundary and TERM-004 abnormal-drain proofs.
Verification always precedes bytecode lowering.

## Determinism and resource limits

Function order follows stable semantic identity. Within a function, blocks,
locals, and temporaries are allocated by the deterministic HIR evaluation
order. Verification never depends on hash iteration.

MIR construction and both dataflow analyses consume explicit request budgets
before unbounded allocation. Function, block, local, statement, and verifier
step limits are part of `CompilationRequest::limits`; exhaustion is the
normative implementation-limit diagnostic `T0002`. Deep source nesting has
already been converted into topological arenas; MIR traversal uses worklists
rather than the Rust process stack.

`Operation::Run` performs this complete lowering and verification before
bytecode construction. Bytecode admission and VM execution repeat their own
independent structural gates; malformed MIR never reaches either boundary.
