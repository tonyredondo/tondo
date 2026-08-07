# Typed slot bytecode contract

**Status:** BC-001 through BC-005, GEN-002 monomorphization, TRAIT-005 static
dispatch, TRAIT-006 opaque results, CAP-001 closed capabilities, CALL-001
uniform named function values, CALL-002 concrete closure environments, CALL-003
closure protocols and synchronous-safe invocation, CALL-004 effect-preserving
closure callables, OWN-001 intrinsic cursor capabilities, OWN-002 affine
transfers and immediate observations, OWN-003 flow availability, OWN-004
complete-slot reinitialization, OWN-005 typed move paths, OWN-006 affine closure
captures, OWN-007 exact closed `CallOnce` rows, BORROW-001 call-local loans, and
BORROW-002 inferred pattern regions, BORROW-003 reborrow permissions, BORROW-004
static collection disjunction, BORROW-005 runtime overlap proofs, BORROW-006
loaned-iterator boundaries, TERM-001 independent terminal classification,
TERM-002 normal-path terminal ownership, TERM-003 explicit defer/guard cleanup,
TERM-004 closed abnormal fallbacks, TERM-005 exact explicit/fallback exclusion,
REF-001 managed identity cells/shared projections, REF-002 identity
equality/keys, ARRAY-001 runtime array length, ARRAY-002 checked array
indexing, ARRAY-003 checked slicing, ARRAY-004 logical slice snapshots,
ARRAY-005 fixed versus structural array mutation, ARRAY-006 closed lifted
arithmetic, ARRAY-007 named concatenation/repetition, ITER-001/002 static user
iterators plus all four intrinsic iteration forms, and the M3 VM admission
path implemented, plus TEXT-001 immutable UTF-8 strings and TEXT-004 distinct
text and byte domains, and TEXT-002 Unicode-scalar String length, indexing, and
slicing, plus TEXT-003 intrinsic/static Display dispatch and interpolation,
plus VARIADIC-001/002 homogeneous final packs and whole-array spread, plus
ASYNC-001..004, EXEC-001/002, SCOPE-001, SPAWN-001, JOIN-001,
CANCEL-001/002, PANIC-ASYNC-001, SEND-001, SHARE-001, and MAIN-ASYNC-001

This document fixes the in-memory boundary between `tondo-compiler` and
`tondo-vm`. It is an implementation contract, not observable Tondo syntax or a
stable artifact format. `TONDO_LANGUAGE_SPEC.md` remains normative.

## Ownership and admission

`tondo-vm` owns the bytecode data model, verifier, and interpreter.
`tondo-compiler` owns deterministic lowering from verified MIR. The dependency
therefore points from compiler to VM: the VM never imports HIR, MIR, resolver
IDs, or the compiler type interner.

`lower_to_bytecode` accepts only MIR that passes `verify_mir`. It converts all
compiler identities to dense request-local indices, builds the complete
program, and invokes the VM-owned `verify_bytecode_with_limits` before
returning. A caller that fabricates or mutates a `BytecodeProgram` must invoke
the same verifier; execution will repeat that gate rather than trust compiler
provenance.

## Program catalogs

A `BytecodeProgram` owns five deterministic tables:

- canonical structural types;
- local nominal declarations and their generic layout templates;
- callable signatures and optional function implementations;
- normalized named constants; and
- executable function bodies.

Type entries preserve scalar, tuple, function, option, result, union,
intrinsic, nominal, generic, opaque, generated, and cursor structure without a
compiler `TypeId`. Nominal metadata records stable identity, generic arity,
record fields, newtype payload, and every enum variant payload. Layout checks
therefore substitute generic arguments from metadata; an instruction cannot
declare a forged field result type and make it valid merely by being
self-consistent.

An executable opaque type entry records its declaration identity, concrete
family arguments, and concrete witness type. The witness is verifier metadata
for checking representation seals; it is not a runtime witness table, value
field, vtable, or reflection capability. Ordinary callable signatures and
consumers continue to name the opaque entry rather than the witness.

The VM data model retains receiver position, parameter modes, variadic element
type, generic arity, outcome, function type, optional implementation, and
optional concrete-closure metadata. Closure metadata records the generated
environment type, ordered capture schema, and `Call`/`CallMut`/`CallOnce` row.
Function entries retain the inferred suspendible effect and the exact `unsafe`
bit. A non-suspendible, suspendible or unsafe callable remains distinct after
compiler `TypeId` values have disappeared; no `async` modifier is serialized.
Compiler-produced executable callable entries are concrete instances: their
generic arity is zero and their signature types have already been substituted.
Static function operands name that concrete callable and carry an empty
type-argument vector. Indirect calls retain a structural concrete function
signature and selected protocol. Named constants are already evaluated and
normalized; execution never invokes arbitrary code to initialize them.

## Monomorphization boundary

`lower_to_bytecode` discovers concrete named and closure callable instances
before allocating any bytecode table. It roots every non-generic callable and
every specialized function value reachable from an evaluated constant, then
transitively scans reached MIR templates for static function operands and
closure aggregate references. Nested type arguments are substituted with the
enclosing instance before their callee is queued.
When a constant retains a qualified source-trait associated function, the same
static selection used for a reached MIR operand chooses its override or default;
the normalized constant stores only that concrete callable ID. Composite
constants are traversed recursively, so nesting cannot hide an executable
function root.
Trait defaults retain a hidden generic `Self` position, even on otherwise
non-generic traits, so declaring a default never makes it an executable root.
Static dispatch must select and specialize that template before it can enter the
worklist. A concrete non-generic implementation method already has an ordinary
checked HIR/MIR body and may enter the bootstrap worklist under its stable
`implementation#N.method#M` identity; generic implementation methods remain
templates until dispatch supplies their header arguments.

For a reached source-trait or prelude-trait operand, monomorphization first
substitutes the enclosing callable's concrete types, reconstructs the complete
trait query, and selects its unique coherent implementation. An explicit
override targets that implementation method; an omitted source-trait method
targets its checked default template. The selected signature is independently
specialized and required to equal the source operand's exact function type
before the target enters the worklist. The resulting bytecode operand is an
ordinary direct function reference with zero generic arguments. No trait
identity, witness table, vtable, or runtime type pack crosses this boundary.

A user `Iterator[T]` loop follows the same rule: its header call becomes a
direct reference to the selected `next` implementation and then ordinary
`Option` discriminant control flow. Only the closed collection protocols use
the VM's intrinsic iterator-state and iterator-next instructions.

Named instances are deduplicated by callable identity plus the complete
concrete argument vector; closure instances use the source closure identity plus
that vector. Direct recursion with the same vector therefore terminates.
Type-expanding recursion creates distinct instances and stops with `T0002` when
the shared generic-instantiation budget is exhausted. A generic closure body
consumes its own unique instance from that same budget. The same failure rule
applies if substitution exhausts the interned specialized-type budget. No
partial program crosses the verifier boundary.

Source-generic protocols are not silently treated as a closed answer. Once the
capture schema is concrete, lowering rederives `Discard` from the bytecode type
and nominal graph and specializes `CallOnce`. This may safely strengthen an
open HIR row when, for example, an unconstrained `T` becomes `Int`; `Call` and
`CallMut` remain fixed by the emitted body operations. The bytecode verifier
then derives the complete row independently from the concrete CFG.

For each reached function, lowering builds a complete template-to-concrete map
covering its signature, locals, places, projections, operands, rvalues,
operations, discriminant tags, and outcome. A missing mapping or a surviving
generic/inference node is an internal construction error. Unreferenced generic
functions have no bytecode body. Equal specializations reached from several
calls or constants share one callable and one function entry.

Opaque families are specialized by declaration identity plus the complete
concrete generic argument vector. Lowering substitutes the compiler-private
witness with that same instance map and retains an explicit `Opaque` coercion
from the concrete representation to the family entry. Equal instances share
one catalog type; different generic arguments remain different opaque types.
Neither specialization nor sealing allocates a wrapper.

Generic nominal metadata deliberately remains a layout template, rather than
being duplicated per use. This is the only generic structure required by
compiler-produced executable bytecode: the verifier substitutes concrete
nominal arguments while validating fields and variants. Executable function
type-use tables themselves are concrete.

## Function tables and slots

Each function owns:

- a strictly ordered set of global type IDs used by that body;
- a sorted, deduplicated source-span table;
- typed frame slots for the return place, parameters, user locals, and
  temporaries;
- a dense loan table whose entries retain one call-local/region kind, one
  non-value parameter mode, and one fixed typed place;
- parameter, entry, unwind, and return-slot indices; and
- basic blocks in deterministic MIR order.

A closure function's parameter slot zero is its generated environment; the
source-visible parameters follow it. Capture projections identify both the
concrete closure callable and capture index, so another environment with a
compatible-looking field type cannot substitute for it.

Every executable item references a function-local span-table index. All spans
remain in the function's source file and use semi-open byte ranges. The
function source span is retained separately for symbolication and diagnostics.

Slots are explicit roots. There is no operand stack whose types or liveness
must be reconstructed at an instruction offset. `StorageLive` and
`StorageDead` reserve the later ownership/cleanup boundary; parameters and the
return place have function-wide storage.

## Trace descriptors

The compiler does not serialize or annotate tracing instructions. After the
ordinary bytecode checks succeed, the VM derives `BytecodeTraceMetadata`
directly from the closed type, nominal, callable, and function catalogs. The
result has:

- one object-shape descriptor at every type ID;
- the ordered capture schema and exact callable identity of each generated
  closure environment; and
- one frame descriptor at every function ID containing its exact slot-type
  vector.

Structural descriptors retain child type IDs, generic nominal arguments,
field/member order, variant payload layouts, union members, cursor mode, and
opaque witness shape. `Inline` means that no heap object may be allocated under
that type descriptor; it does not make a frame slot a static root bitmap,
because callable erasure can store a managed closure in a function-typed slot.
The bootstrap VM inspects the tagged live value while using the frame
descriptor to prove the slot schema.

Derivation is part of admission and fails closed on unknown references, wrong
intrinsic arity, unknown nominal layouts, duplicate or non-generated closure
environments, cyclic opaque witnesses, and invalid frame slot types. The heap
stores the descriptor type ID with each object and validates every allocation
and replacement against the derived shape before tracing it. Consequently,
adding a constructible managed bytecode form requires extending this exhaustive
derivation and its negative tests; object-local tracing is not authoritative.

## Instructions and control flow

Ordinary instructions perform storage lifetime changes, reserve/release one
loan identity, store one pure typed rvalue, register one deferred invocation,
or retarget/disarm its unique affine guard. Rvalues cover loads,
copies/moves, constants, pure arithmetic,
construction, record update, coercion, closed numeric conversion, range, membership,
length, and iterator-state creation. The latter accepts an intrinsic collection
`C` and produces only its distinct `cursor[own,C]`, `cursor[ref,C]`, or
`cursor[mut,C]` type; `IteratorNext` accepts that cursor rather than a
collection-shaped alias. Ref and mut cursors additionally require a `Borrow`
source operand with the exact root-loan mode, while an own cursor rejects one,
so a backend cannot silently replace observation or mutation with a copy.

`NumericConversionError` has an independently known intrinsic descriptor with
three unit variants. Its verified ordinals are `OutOfRange = 0`,
`NotFinite = 1`, and `NotIntegral = 2`; constants, aggregates, and branch tags
reject any other ordinal or payload shape. Checked conversion must return
`Result[target, NumericConversionError]`, while identity and total conversion
must return the target directly.

Integer arithmetic and signed negation use the checked invocation path.
Division or remainder by zero, signed-minimum division by `-1`, and every
out-of-range arithmetic result retain distinct normative panic classes. Shifts
use that path only to validate the right operand; once valid, they transform the
left operand's fixed-width bit pattern. Left shift discards high bits, signed
right shift extends the sign bit, and unsigned or `Byte` right shift introduces
zeroes. The verifier requires an integer non-`Byte` count and preserves the
left type exactly. Bitwise operands must have that identical integer or `Byte`
type.

Named integer constants are admitted only when their mathematical value fits
their exact scalar descriptor. `Byte` uses that same checked constant spelling
in bytecode but materializes as the VM's distinct byte value, never as `Int`.

Named floating constants store the bits of the VM's canonical `f64` envelope.
A non-NaN `Float32` value is admitted only when those bits are the exact
binary64 widening of one binary32 value; NaN payload bits are deliberately
non-normative. Immediate float spellings must parse finitely at their declared
precision and may not carry the opposite precision's suffix. Runtime-generated
infinity and NaN therefore remain values, while a source literal that rounds to
infinity is rejected before execution.

Immediate `String` and `Char` constants retain their source spelling so tooling
can reproduce the program, but admission decodes that spelling independently.
The verifier rejects malformed delimiters, escapes, Unicode scalar values, and
surrogates before execution. A verified string therefore always materializes
as valid UTF-8 and a verified character as exactly one Unicode scalar.
`String`, `Char`, `Byte`, and `Array[Byte]` have distinct descriptors and no
representation-preserving coercion between them. Bytecode intentionally has no
intrinsic `Bytes` descriptor; encoded buffers remain a future standard-library
abstraction over language-level values.

An Array aggregate stores its complete ordered operand list while its result
type is only `Array[T]`; operand count is runtime data, not type metadata.
`Length` is the internal shape observation used by array-pattern lowering and
the future String length API. The verifier independently requires an
`Array[T]` or `String` operand and an `Int` result, rejecting every other
scalar or collection before execution. Copy, call, and return instructions
preserve the complete value rather than carrying a separately trusted length.

For a ref cursor, `IteratorNext` writes an `Int` position rather than an owned
item and carries the exact borrowed source place. `IteratorElement { index }`
then resolves that position against the original `Array`, `Map`, or `Set`.
Array and set elements remain in their collection; map tuple fields are views
of the stored key/value pair rather than ownership transfers. The verifier
requires one canonical cursor definition, source region, position producer,
and matching element projection.
An own cursor instead transfers each yielded element out of its private
collection state. Arrays and sets move one element; maps move one key/value
pair into the yielded tuple. The remaining collection stays in the cursor so a
guarded early exit cleans only that remainder. Natural exhaustion may carry an
edge-specific guard disarm as described below.
Copy and move forms ordinarily come directly from verified MIR.
Monomorphization preserves that source-generic decision: a move selected in a
`T: Discard` body remains a move even when one concrete instantiation happens
to substitute a `Copy` type. The one cleanup-specific exception is a guarded
defer operand whose closed type gains `Copy`: lowering converts its exact
guarded `Move` into the required registration-time `Copy` snapshot, removes the
now-vacuous guard transitions, and re-verifies the complete bytecode program.
The VM verifier independently rejects every forged `Copy` whose closed concrete
type lacks `Copy`.

Every `Move` consumes its typed place. The verifier requires live storage and an
available path, then records that root or projection as unavailable. Reads and
later moves reject an unavailable ancestor, descendant, or overlapping path;
statically disjoint record fields, tuple slots, variant payloads, and
array-pattern regions remain independent. Dynamic indices and slices overlap
conservatively. Joins union unavailable paths across every normal, unwind, and
loop predecessor, so a value cannot be reused merely because it survives on
one incoming edge.

A direct store defines its destination whether that slot held a value or was
made unavailable by an earlier verified move. `ValidatePlaces` therefore does
not emit a root read for an unprojected write destination. A projected write
requires an available parent and restores only its typed subtree. This is the
backend distinction that lets OWN-004 reinitialize a complete `var`, supports
compiler-internal atomic replacement, and still prevents a source program from
reviving an owner through an unproved partial write.

The parameter and loan mode remains available at execution. A root store
reached through `mut Array[T]` is a fixed-extent publication even when its
lender is a field, element, reborrow, or slice; the VM compares the current and
replacement lengths before writing. A root store through `var Array[T]` may
instead replace the complete owner with another length. Bytecode types prove
both sides are the same `Array[T]`; their runtime lengths are intentionally not
encoded in the type or instruction stream.

`BytecodeAggregateKind::Ref` constructs one managed identity cell from exactly
one operand whose type is the target of the result `Ref[T]`.
`BytecodeProjectionKind::RefValue` resolves its payload as a shared, read-only
place. It may appear in `Copy` only when the closed target is `Copy`, or in a
shared loan used by an immediate borrowed call. The verifier rejects a `Move`;
a `Store`, `Invoke`, `IteratorNext` or write-validation destination; or a
`mut`/`var` loan containing the projection, including nested paths. Copying the
`Ref[T]` root itself remains an ordinary closed `Copy` and preserves the cell
identity.

A closure construction is an ordinary managed aggregate whose result is a
concrete generated type. Its shape names one concrete closure callable; that
callable owns the identical environment type, ordered capture schema, protocol
row, effect-exact function signature, and lowered body. Its operands carry the
corresponding concrete capture values and preserve MIR's contextual Copy or Move
access. Verification requires identity, schema, signature, value, capability,
and path-availability agreement before allocation. Constructing this aggregate
does not invoke its body, including for async or unsafe closure kinds.

A call operation accepts either a direct concrete function operand or a
borrow/copy/move of a place containing a callable value. The latter is the
uniform indirect-call path used by concrete closures, generic or opaque
callables, parameters, locals, fields, and named constants. It carries the exact
structural function signature and selected protocol. Before execution the
verifier resolves the concrete callable representation and requires exact
modes, arity, variadic shape, outcome, function signature, protocol support, and
access form. A source protocol exposed by a generic or opaque contract is
normalized to the strongest safe concrete specialization without changing
whether the source operand borrows, copies, or moves.
Each individual variadic operand retains a `VariadicElement` target, value mode,
exact element type, and Copy or Move access. Zero operands are represented by
the variadic function shape itself rather than a synthetic source argument.
Direct callables, methods, closures, generic instances, and indirect uniform
function values use this same encoding. Verification rederives the unique final
variadic element from callable metadata and the structural function signature;
a forged fixed target, receiver, mode, type, or non-variadic association is
invalid bytecode.
One explicit spread instead retains a unique final `VariadicSpread` target and
an exact by-value `Array[T]` operand. Its Copy or Move access is rederived from
the closed array capability. The VM receives the complete logical snapshot or
affine owner and transfers its items into the body-visible pack without
recopying each element.
Value arguments use copy/move access. Each `ref`, `mut`, or `var` argument uses
one `Loan(id)` operand whose table entry has the identical mode, type, and fixed
place and `CallLocal` kind. `ReserveLoan(id)` begins its active interval after
that argument's place is resolved. The call consumes all of its call-local loan
operands atomically; `ReleaseLoan(id)` closes reservations abandoned while a
later argument executes `return`, `fail`, `?`, `break`, or `continue`.

A pattern `ref` uses a shared `Region` entry. Every place reached through that
binding retains a `source_loan` anchor, including nested region bindings and
call-local reborrows. MIR has already inserted releases at statement or
edge-specific last uses; bytecode lowers those identities and bridge blocks
without recomputing source liveness. The bytecode verifier nevertheless proves
the region kind and mode, acyclic source order, containment within the reserved
path, active source chains, and the prohibition on consuming a region as a call
operand. It propagates the exact active set through branches and loops and
rejects mismatched joins, duplicate or inactive events, and overlapping
exclusive access. A source region must remain open until every transitive child
reservation has closed.

Execution defensively repeats the dynamic part of that contract: every read,
take, write, place validation, reservation, and call reached through an anchored
place requires its complete shared source-region chain to remain active. A
write or move through a shared anchor is rejected even if malformed bytecode
somehow bypasses the admission gate.

Panic unwinding discards every reservation in the abandoned frame, so cleanup
blocks cannot reserve or release loans.

Reborrowing preserves the same permission order as HIR/MIR, including `var`
from a complete strict projection of `mut`. `BytecodePlace` owns the canonical
structural-replacement classification shared by verification and execution: a
root, slice, array rest, potential map entry, or opaque projection cannot
perform that upgrade, while a fixed aggregate payload or existing array element
can. Record fields and tuple slots may be proved disjoint. BORROW-004 admits
loan metadata containing index, slice, array-pattern element, and array-pattern
rest projections. The verifier recovers integer constants only from
single-definition temporary slots and independently rederives interval, stride,
and pattern-region disjunction. BORROW-005 preserves runtime-dependent relations
as explicit, canonical conflict IDs: `ValidateLoan` protects a reservation;
`Index` and `Slice` operations protect their atomic read; and every
`ValidatePlaces` entry has one aligned list protecting its following read or
write. Statically disjoint relations carry no conflict ID, while inevitable
overlap remains invalid bytecode.

BORROW-006 treats `IteratorNext` as an explicit loan boundary. The verifier
allows shared `Region` loans and the cursor's complete canonical source chain
in its incoming active set. Every exclusive region must belong to that chain;
call-local loans and unrelated exclusive reservations are rejected. It
preserves the set on both normal successors and clears it on unwind. The VM
advances either loaned cursor by position without calling the owning copy path,
checks that its runtime source is the terminator's anchored collection, and
resolves each element under the active region chain. Exclusive map projections
are limited to the value field, and array replacement enforces `mut` versus
`var` extent at the write boundary. `Await` reuses the same incoming-set proof,
rejects call-local and exclusive loans, and preserves only the
verifier-approved suspension-live set. `Spawn` instead admits its exact
structured `ref` loans and binds them to the resulting `Join` lifetime.

`Borrow` remains a separate non-storable form admitted only for equality,
membership, length, discriminant branches, index/slice collection bases,
indirect shared/exclusive callees, intrinsic ref/mut-cursor construction, and
the replacement witness attached to write validation. Stores, aggregates,
returns, every call argument, and unrelated operations reject it.

The bootstrap `Call` operation remains deliberately synchronous. Its signature
must have the suspendible-effect bit clear and its explicit `unsafe_call` bit
must equal the callable's unsafe effect. `Await` and `Spawn` are the only
suspendible initiation terminators and rederive their operation's effect, protocol, outcome, arguments,
capabilities, and control-flow contract, including the same unsafe-bit
agreement. HIR supplies that bit only after proving a lexical unsafe region.
The six raw Pointer host operations keep separate enum identities; the verifier
rederives their concrete Pointer element, `Int` offset, `UInt64` address, value,
and result types before execution.

`BytecodeCoercion::Opaque` and `BytecodeCoercion::CallableErasure` are verified
runtime no-ops: execution forwards the already materialized value unchanged.
The latter is admitted only from an exact `Call` closure whose environment
proves `Copy + Send + Share` to the identical structural `fn(...)` signature.
Their distinct opcodes preserve proof boundaries and cannot be exchanged with
another coercion kind without invalidating the program.

Potentially panicking work remains a terminator-level `Invoke` with explicit
normal destination/target and cleanup target. This includes checked arithmetic,
named array sequences, map construction, indexing, slicing, calls, `assert`,
and `panic`. Other
terminators cover direct branches, boolean and discriminant dispatch,
iterator-next, atomic destination validation, loan validation, return, panic
resumption, scope-specific defer drain, unified abnormal drain, and unreachable
code. Every potentially panicking edge in one function targets the same empty
`DrainUnwind` cleanup block, whose only successor is the distinguished
`ResumePanic` block.

`RegisterFallback` arms one closed structural action for a concrete `Present`
owner. Monomorphization removes registrations whose generic MIR owner closes as
`Absent` and rejects `Potential` executable state. Entry parameters and closure
capture slots register before ordinary instructions; terminal stores,
successful invocation results, and owning iterator values register on the
exact materialization edge. `RetargetCleanup` and `DisarmCleanup` are shared by
explicit guards and fallback owners. Registering an explicit guard removes the
enclosed fallback only after capture succeeds, so a failed registration leaves
the original entry armed. The independent verifier requires a concrete
terminal guard to replace exactly one fallback and rejects any later overlap.
The VM validates multiplicity before mutating its ledger, removes the fallback,
and only then appends the explicit entry; captured temporary roots are released
on every success or failure path.

`DrainDefers` selects only explicit entries in the abandoned lexical scopes.
`DrainUnwind` instead pops the unified vector in exact LIFO order, including
structural fallbacks. The runtime walker consumes tuple, option, result, union,
array, map, set, range, nominal, closure-environment, and owning-cursor state in
reverse construction order, replacing extracted heap fields with holes so a
transferred child cannot be consumed twice. A normal return discards fallback
markers without executing them only after the verified TERM-002 source proof;
it still rejects any explicit entry.

The walker dispatches a direct terminal root through the sealed bytecode
registry. Its only current action is `JoinTeardown`, declared as may-suspend.
An active `Join` carries one child task and owning scope identity. Teardown
requests cancellation when needed, parks the owner until the child reaches a
terminal state, consumes the completion exactly once, and recursively tears
down terminal values it owned. The owner remains parked before later lexical
defers execute. Malformed bytecode cannot fabricate, duplicate, detach, or
consume a handle from another scope.

`ValidateLoan`
must resolve to a normal block
whose first instruction reserves the same loan. A read validation aligns each
destination with no replacement;
a write validation requires one borrowed replacement witness of exactly the
place type. If the normalized effective path ends in a slice, including when
that slice is hidden behind a borrowed callee parameter, the VM compares its
length with the witnessed `Array` and can raise `P0006` before the first store
without consuming the later write operand. The verifier propagates active loans
and pending proofs together, rederives every `against` list, rejects missing,
extra, duplicated, inactive, or noncanonical IDs, and prevents any pending
index/bound slot from changing before its proof is consumed. Missing or
misaligned metadata is invalid bytecode.

Places start at one slot and carry typed projections. Projections include
record/newtype fields, the shared `RefValue` payload, tuple positions,
enum/option/result/union payloads, array-pattern segments, dynamic indexing,
and slices. Index and bound operands are slots evaluated earlier, preserving
MIR evaluation order.

Every array `Index` operation or projection has an `Array[T]` base, an `Int`
operand, and result `T`; the verifier rederives all three facts. Bytecode keeps
the original signed operand. `normalize_array_index` is the single checked
normalizer shared by runtime value reads, place reads/moves/writes, loan paths,
and compiler constant evaluation. It computes negative indices by distance from
the end, avoiding an overflowing signed `n + i` intermediate.

Every array `Slice` operation or projection has an `Array[T]` base, zero to
three `Int` operands, and result `Array[T]`; the verifier rederives that complete
shape and rejects forged bound types before execution. Bytecode preserves
omitted start, end, and step as independent `None` values.
`normalize_array_slice_indices` is the single normalizer shared by compiler
constant evaluation and every VM slice path. It applies sign-dependent defaults,
offsets only explicit negative bounds, clips them to the direction's exact
domain, and checks remaining distance before advancing, so even `Int.min` cannot
overflow. A zero step maps to `P0002`; an omitted negative end remains distinct
from an explicit `-1`.

Every `Slice` operation is a materialization boundary, so the verifier also
derives the closed `Copy` capability of its complete `Array[T]` result. A slice
projection retained only in call-loan metadata has no such requirement and can
name affine elements. Forging a materializing operation over non-`Copy`
elements is invalid bytecode, independently of the HIR check.

String access reuses the signed normalizers without pretending UTF-8 is an
array. `BytecodeIndexAccess::String` requires exactly `String + Int -> Char`;
a String `Slice` preserves `String` and accepts the same three optional `Int`
bounds. Both forms are checked `Invoke` operations, so `P0001` and `P0002`
follow the ordinary unwind edge. The verifier rejects the String access tag and
every String slice inside a place projection: text is immutable and these
operations always materialize values. The internal `Length` rvalue accepts only
`Array` or `String`, returns `Int`, and delegates the concrete count to the
verified runtime shape.

Monomorphization resolves user `Display` calls to the same concrete callable
path as every other open prelude trait. A scalar or `String` specialization is
instead closed to `BytecodeOperationKind::Display`, which consumes exactly one
active call-local `ref` loan and returns `String`; there is no callable lookup,
vtable, or runtime type pack. `BytecodeRvalueKind::Interpolate` accepts only
`String` operands and exactly one more decoded segment than operand. The
verifier rederives both contracts and accounts for the intrinsic call's loan
consumption, so forged modes, associations, types, arities, or loan lifetimes
cannot reach execution.

`BytecodeOperationKind::ArraySequence` preserves the closed `Concat`/`Repeat`
tag and both ordered operands. Its result and borrowed receiver must be the
same `Array[T]`, `T` must satisfy the independently derived closed `Copy`
capability, and the value argument must be that same array type for `Concat` or
the canonical `Int` type for `Repeat`. It is valid only inside `Invoke`; a
forged pure rvalue, `Copy` receiver, loan receiver, borrowed argument, changed
tag, or mismatched type is rejected before execution.

The VM resolves each protected path to normalized runtime components: negative
indices use the current array length, slices become their exact selected-index
sets, and map projections snapshot their key value. It compares only the active
loans named by verified `against` metadata. Actual intersection raises `P0004`;
disjoint data proceeds without changing evaluation order. Bounds, absent
borrowed map entries, and zero slice steps retain their own panic classes before
the callee runs, and unwind clears every reservation in the abandoned frame.

Map construction includes an explicit reject-versus-replace flag for dynamic
duplicate keys. The VM evaluates the already-materialized entry operands in
order, detects duplicates before allocating the final map, and either preserves
the first insertion position while replacing its value or raises `P0009`.
Structural equality preserves sequence order for tuples and arrays, but compares
maps and sets by membership rather than insertion order. It is emitted only for
an identical type proven `Equatable`. Two `Ref[T]` operands are equal exactly
when their managed handles identify the same live cell; distinct cells never
compare their payloads. Map lookup/replacement and set membership reuse that
same equality, which supplies `Key` by identity independently of `T`.

Set aggregate verification requires one `Set[K]` result, one `K` operand per
source entry, and `K: Key`. Runtime construction keeps the first equal entry and
therefore preserves its insertion position even when a later duplicate is
dynamic. Range construction carries an explicit exclusive/inclusive tag and
two endpoints of the exact `T` in `Range[T]`; containment and iteration must
consume that tag rather than synthesize an adjusted endpoint.

`BytecodeRvalueKind::MapRemove` mutates only a `Map[K, V]` place sourced from
its exact active `var` region and returns `Option[V]`. The VM searches with the
same normative key equality as lookup, removes the matching insertion-order
entry without reordering its neighbours, and roots the transferred value across
heap replacement and option allocation. An absent key allocates `none` without
changing the map. Bytecode verification rejects a changed key/result type,
non-map receiver, weaker region, or forged region origin before execution.

## Independent verification

Before execution, the verifier proves:

- every type, nominal, callable, constant, function, slot, span, block, and
  pool index exists;
- catalogs, local type tables, span tables, implementations, and parameter
  tables are unique and internally linked;
- type constructors, generic arities, nominal fields/variants, constants,
  projections, aggregates, operators, conversions, iterators, and tags have
  their exact structural types;
- every `Ref` construction has one exact target operand, and every `RefValue`
  path is read-only, shared-only, and immovable;
- every closure callable has a unique generated environment, executable body,
  hidden environment parameter, exact capture schema, and protocol row; closure
  aggregates and capture projections name that same callable and match every
  operand exactly;
- closure protocols are rederived from the executable body and cannot be
  strengthened by forged catalog metadata; a body that moves an environment
  path cannot advertise `Call` or `CallMut`, and an async body that writes its
  environment cannot advertise either borrowed protocol; `CallOnce` requires
  every non-`Discard` capture to be completely moved on every reachable normal
  return, with branch states intersected rather than unioned;
- async callables have no `mut` or `var` parameter; the synchronous-safe call
  opcode rejects every async or unsafe function signature, while `Await` and
  `Spawn` require async-safe signatures and their exact logical outcomes;
- every closed executable `Map[K, V]` and `Set[K]` has `K: Key`, every `Ref[T]`
  has `T: Discard`, equality has `T: Equatable`, array membership has an
  equatable element, map/set membership has a key, and map lookup has `V: Copy`;
- each opaque `(identity, concrete arguments)` family occurs once, contains no
  executable generic parameter, has a non-`Never` witness, and participates in
  no direct or mutual representation cycle;
- the sealed direct terminal registry contains `Join` with its one `await` and
  structured-teardown contract; terminal presence is rederived structurally
  through concrete types and nominal templates, every present type is
  non-`Copy` and non-`Discard`, and an opaque witness cannot hide a terminal
  token;
- every opaque coercion seals exactly its catalogued witness into the matching
  opaque family;
- calls and async operations have an exact structural signature, matching
  outcome, complete fixed/receiver association, correct modes, valid variadic
  element or final spread, supported protocol, protocol-compatible
  loan/copy/move access, and no unimplemented unsafe effect;
- a generic or opaque callable resolves to one concrete named function or
  closure with the same signature, while a callable erasure preserves the
  concrete closure value and exact uniform function signature;
- `Borrow` is confined to the enumerated immediate observation positions and
  cannot escape into any call argument, slot, aggregate, return, or unrelated
  operation;
- each loan has exactly one static reservation; a call-local loan has at most
  one static call consumer while a region has none; every reachable path
  consumes or explicitly releases an active reservation, anchored places use
  an active shared source chain, active sets agree at joins, overlapping
  exclusive regions are rejected, and `Loan` cannot occur outside its matching
  non-value argument;
- each `Spawn` names the active innermost task scope, every task-scope stack
  agrees at CFG joins, and `DrainScopes` removes exactly an active
  inner-to-outer suffix before its aligned defer scopes;
- every `Await` has one valid async call or affine `Join` operand, writes its
  logical result only on the normal edge, and admits only `Send` live values
  plus the current scope's sealed join exception;
- every `RegisterDefer` contains one infallible `Unit` operation. Synchronous
  entries use the `Deferred` call context; `defer await` entries use
  `DeferredAsync` and therefore retain exactly one async call signature. Both
  snapshot all closed `Copy` operands, retain at most one complete affine guard
  in a local or closure-capture owner slot, and belong to a live lexical scope;
- every concrete terminal entry parameter/capture and every terminal store,
  successful invocation result, and iterator-value edge has an immediate
  fallback or cleanup retarget;
- the independent cleanup dataflow rejects duplicate live registrations,
  terminal explicit guards that do not replace exactly one fallback,
  explicit/fallback overlap or rearming, partial or embedded explicit-guard
  moves, non-immediate retarget/disarm transitions, post-drain access before
  complete reinitialization, incompatible joins, incorrect scope drains,
  explicit entries at `Return`, and any entry at `ResumePanic`;
- an own intrinsic `IteratorNext` has an exhaustion guard exactly when its
  closed collection status is `Present`; the guard is the exact cursor-source
  path and is removed only on the exhausted edge, while ref cursors never carry
  one;
- normal edges remain in normal code, unwind edges enter cleanup code, and the
  distinguished unwind block resumes panic;
- all reachable reads have a dominating live definition, every root or
  projected move consumes exactly one available typed path on each CFG route,
  edge-produced values exist only on their successful edge, and the return slot
  is initialized;
- payload projections are dominated by their matching discriminant edge and a
  potentially overlapping write invalidates that refinement; and
- every `assert` retains a nonempty condition representation for its default
  runtime message; and
- unreachable retained blocks contain no executable bytecode.

Initialization, move-path availability, and lifetime share one forward dataflow
analysis; discriminant refinement remains separate. Both have an explicit
shared step budget. Exhaustion is a resource limit, not malformed source and
not permission to execute partially verified code.

These capability checks are derived again from the bytecode type graph and
generic nominal layout summaries; generated closure types derive `Copy`,
`Discard`, `Send`, and `Share` componentwise from their capture schema and never
derive `Equatable` or `Key`. Owned intrinsic cursors derive those same four
capabilities from their collection. Ref cursors always derive `Copy + Discard`
and derive both `Send` and `Share` only from `C: Send + Share`; neither cursor
mode derives `Equatable` or `Key`. The VM does not trust the HIR status table or
receive runtime capability objects. Generic template parameters are admitted
only because HIR has already proved their contextual bounds, and every reached
executable specialization is closed before this verifier consumes it.

Terminal status is derived by a separate existential graph because absence of
`Discard` is not itself proof of one concrete terminal root. The VM catalog
uses the same `Absent`/`Potential`/`Present` lattice as HIR, but computes it
without importing compiler metadata. Nominal summaries retain only the generic
positions that can carry a token; own cursors and closure environments follow
owned state, while ref cursors and safe/raw references do not acquire ownership.
Executable opaque witnesses must be `Absent`. TERM-002 consumes the corresponding
HIR proof before executable lowering and the bytecode move verifier preserves
the admitted handoffs. TERM-003 adds `RegisterDefer`, `RetargetCleanup`,
`DisarmCleanup`, and `DrainDefers`, with a separate exact verifier ledger rather
than trusting MIR. Monomorphization resolves a generic intrinsic iterator's
`Potential` exhaustion marker against the closed catalog: it retains the marker
for `Present`, removes it for `Absent`, and rejects any remaining `Potential`
executable state. TERM-004 adds `RegisterFallback` and `DrainUnwind`, closes the
same specialization for fallback owners, and makes the VM execute structural
fallbacks from the sealed registry. TERM-005 independently requires every
terminal explicit guard to replace exactly one fallback and rejects rearming
the fallback after replacement. M7 closes `JoinTeardown` with active task state,
suspendible scope drains, cooperative cancellation, and exact-once completion
consumption.

## Determinism, limits, and tooling

Catalogs follow stable HIR/MIR order; instance sets, type-use sets, and span
tables are sorted. No observable ordering depends on hash iteration.
Construction bounds generic instances, specialized and catalog types, nominals,
callables, constants, functions, per-function slots, blocks, instructions, and
spans. Driver exhaustion becomes `T0002`.

`disassemble` renders deterministic human-readable text for tests and debugging
and labels itself tooling-only. It prints closure schema/protocol metadata and
an opaque declaration identity plus family arguments, but deliberately redacts
the private opaque witness relation. There is no bytecode serializer or loader
in the bootstrap. The text, enum layout, dense indices, and Rust representation
may change without compatibility guarantees.
