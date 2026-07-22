# Typed HIR to MIR contract

**Status:** M3 typed CFG plus M4 uniform function values, four effect-preserving
Copy closure forms, executable synchronous-safe closure calls, static-trait and
opaque-result lowering, OWN-001 intrinsic cursor state, OWN-002 affine
transfers, and verification implemented

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
- every indirect call has one exact synchronous-safe signature, selected call
  protocol, and source access form accepted by typed HIR;
- every prelude trait operand has complete canonical arguments and the exact
  `Display.display` or `Iterator.next` function type;
- every ordinary named-function operand is either intrinsically non-generic or
  carries one complete specialization whose exact substituted signature is its
  operand type;
- every iterator loop records either a valid intrinsic source plus its exact
  `cursor[own,C]`/`cursor[ref,C]` state type, or one exact `Iterator[T]`
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
| Typed HIR | Static types, contextual conversions, opaque contracts and witnesses, effect-exact concrete closure signatures, capture sets and call protocols, selected synchronous-safe call access, value/place category, pattern coverage, source evaluation order, and source-level control targets |
| MIR construction (M3/M4) | Typed locals and temporaries, explicit CFG, places, synchronous-safe calls, effect-preserving closure bodies with a hidden environment, Copy closure-environment construction, branch targets, normal/abnormal edge shape, and spans |
| Ownership MIR (M5) | Contextual `Copy` versus `Move` and immediate non-escaping observations; later M5 steps add availability, regions, confirmed transfers, cleanup actions, and dynamic overlap checks |
| Async MIR (M7) | Suspension points, resume/cancel/unwind edges, live frame state, and `Send` checks across suspension |
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

Operands distinguish constants, copy reads, move reads, and immediate borrows.
OWN-002 chooses `Copy` or `Move` from the HIR capability graph under the exact
generic bounds of each body. A `T: Copy` body copies; an unbounded or merely
`T: Discard` body moves. The decision is cached per body and type, retained
through monomorphization, and rederived by the MIR verifier. OWN-003 adds the
flow fact that a moved place is unavailable afterwards. A backend never decides
between copy and move from runtime representation alone.

Branches use block IDs, not nested expression nodes. `Never`, `return`, `fail`,
`break`, `continue`, propagation, and panic paths end in terminators without an
invented normal successor. A block is never left unterminated, including
syntactically unreachable blocks retained for spans or diagnostics.

The M3 lowering covers every expression admitted by complete typed HIR,
including short-circuit operators, all three loop forms, exhaustive patterns
and guards, assignment, construction and update, collections, indexing,
slicing, numeric conversions, calls, and both `Option` and `Result`
propagation. Recovery and incomplete interpolation nodes cannot cross the HIR
admission boundary and therefore have no executable MIR interpretation.

A concrete closure expression lowers to one aggregate with its `HirClosureId`
and captures in the exact HIR table order. CALL-002 admits only captures whose
`Copy` proof is closed, so each operand is an unprojected copy of the MIR local
that represents that exact outer `LocalId`. The aggregate result retains the
effect-specific generated closure type. Its independently rooted body becomes a
`MirFunctionId::Closure` function. Slot zero is a hidden environment parameter;
capture references are typed projections from that slot, and explicit source
parameters follow it in their original order. Construction remains separate
from body execution. The body and exact function signature may represent sync,
unsafe, async, or async-unsafe source effects even though only a synchronous-safe
body can currently be reached by a call operation. OWN-006 later replaces the
bootstrap Copy-only boundary with availability-checked moves for affine
captures.

An indirect closure call carries the exact protocol selected by HIR. `Call` and
`CallMut` read a place through a shallow, non-escaping `Borrow` operand so body
updates observe the original environment; `CallMut` additionally requires the
source place to be writable. `CallOnce` uses the ordinary Copy or Move operand
selected by source access, including a non-`Copy` generic or opaque callable.

`Borrow` is not yet a general MIR loan. OWN-002 uses it only where one operation
must observe a place without transferring ownership: equality, membership,
length, discriminant branches, the collection base of index/slice, an indirect
`Call`/`CallMut` callee, a `ref`/`mut`/`var` call argument, and the replacement
whose length is checked before a slice write. A borrowed call argument is
required for every non-value parameter and forbidden for a value parameter. It
may never be stored, returned, inserted into an aggregate, or used by an
unrelated operation. `Iterator.next` uses the same immediate borrow for its
mutable state receiver. A `cursor[ref,C]` source will use this operand once
BORROW-001 admits the source body and proves the longer loop region. The
ordinary MIR call operation rejects an `async` or
`unsafe` function signature. M7 and M9 must introduce and verify their own
effect-aware initiation/context forms rather than weakening that operation.

Checked operations use `Invoke`; indexed and sliced reads therefore cannot
bypass their bounds/unwind edge. Assignment first resolves all destination
places, then materializes its complete RHS, then validates overlap, bounds, and
slice replacement lengths before performing any write. Compound assignment
uses an access validation before reading its previous value and validates the
fully computed replacement again before storing it. Static callees remain
callable operands instead of being erased into ordinary temporaries, preserving
the selected declaration, receiver mode, generic specialization, and variadic
argument association. Source-trait calls retain their specialized trait member;
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

An opaque success exit remains an explicit coercion rvalue whose kind is
`Assignability::Opaque`. MIR preserves both operand and destination types, so a
later phase never needs to rediscover the hidden representation. The coercion
has no runtime transformation: its purpose is to keep the declaration-owned
seal auditable across the typed CFG. For a fallible function the ordinary
`Result` construction and propagation remain outside that success seal, so the
visible error channel is unchanged.

Intrinsic `for` sources use an iterator-state rvalue whose operand is the
collection `C` and whose result is the distinct concrete
`cursor[own,C]`/`cursor[ref,C]` local consumed by `IteratorNext`. The verifier
rejects both a cursor disguised as its collection and a cursor whose mode or
collection differs from typed HIR. A user `Iterator[T]` source is evaluated once
into a state local; each header invokes the typed `Iterator.next` operand with its
immediately borrowed mutable receiver, observes the returned `T?` discriminant,
projects the dominated
`Option` payload, and then binds the irrefutable loop pattern. The MIR shape
therefore exposes every evaluation and edge without treating a user iterator
as a VM intrinsic.

The current admitted bootstrap subset forms only `cursor[own,C]`. Typed HIR
already retains `cursor[ref,C]` and its closed capabilities, but keeps that body
incomplete until BORROW-001 can lower the source as a real shared `Borrow`
operand and prove its region; it is never approximated with a collection copy.

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

M5 populates those blocks with terminal fallback, guard, `defer`, and confirmed
handoff operations. The representation enforces one armed action per terminal
token and disarms before execution. Bytecode lowering preserves these edges; it
does not synthesize destructor behavior.

M7 represents `await` and structured teardown with a suspension terminator.
Its successors distinguish resume, cancellation, and panic/unwind. Values live
across that terminator become explicit frame locals. An exclusive loan may not
be live there, and all surviving values must satisfy the required `Send`
contract before bytecode generation.

## MIR verification layers

The structural verifier introduced in M3 proves at minimum:

- every block has one valid terminator and every successor exists;
- local, field, variant, function, and constant indices are in range;
- every operand and destination agrees with the declared local/type table;
- every use is dominated by a definition and no local is read outside its
  declared storage lifetime;
- place projections are legal for their base type;
- call arity, modes, argument types, and outcome agree with the selected
  callable, and every indirect call repeats the exact HIR signature/protocol
  selection for concrete, generic, and opaque callees;
- the ordinary call operation has neither an `async` nor `unsafe` signature;
  retaining such a callable or body does not make it synchronously executable;
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
- a closure aggregate names existing HIR metadata, has the exact generated
  result and capture layout, and copies each capture from the corresponding
  unprojected outer source binding rather than a merely type-compatible value;
- every closure has exactly one body function with its generated environment as
  hidden parameter zero, exact explicit parameters, capture projections, and
  outcome, while no ordinary function may forge that shape; all four effect
  signatures are retained unchanged;
- `Borrow` appears only in an enumerated immediate observation, as an indirect
  `Call`/`CallMut` callee, in a non-value call argument, or as the exact source
  of `cursor[ref,C]`; it never escapes into storage, value arguments,
  aggregates, returns, or arbitrary rvalues, and `CallOnce` never uses it;
- equality, collection membership, and map lookup satisfy the `Equatable`,
  `Key`, or `Copy` requirement recorded and independently verified in HIR;
- a variant, union, option, or result payload is read only on an edge dominated
  by the corresponding discriminant test, and writes invalidate refinements;
- cleanup edges enter cleanup blocks and cleanup blocks cannot return to an
  abandoned normal path; and
- source spans remain attached to locals and every executable operation, and
  stay within the function's source file.

Definite initialization and storage lifetime are forward dataflow properties,
not assumptions made by bytecode generation. Parameters are initialized at
entry, edge-specific results are initialized only on their successful edge,
and the return place must be initialized on every `Return`. Payload refinement
is a separate forward analysis so initialization alone cannot authorize an
invalid projection.

M5 and M7 extend that proof with ownership, region, terminal-token, and
suspension invariants. Verification always precedes bytecode lowering.

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
