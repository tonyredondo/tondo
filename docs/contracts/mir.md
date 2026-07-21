# Typed HIR to MIR contract

**Status:** M3 typed CFG lowering and verification implemented

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
| Typed HIR | Static types, contextual conversions, value/place category, pattern coverage, source evaluation order, and source-level control targets |
| MIR construction (M3) | Typed locals and temporaries, explicit CFG, places, calls, branch targets, normal/abnormal edge shape, and spans |
| Ownership MIR (M5) | `Copy` versus `Move`, availability, loans and regions, confirmed transfers, cleanup actions, and dynamic overlap checks |
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

Operands distinguish constants, copy reads, and move reads. During M3 the
bootstrap subset can classify only values whose capability is already closed;
M5 completes the classification before admitting affine programs. A backend
must not decide between copy and move from runtime representation.

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

Checked operations use `Invoke`; indexed and sliced reads therefore cannot
bypass their bounds/unwind edge. Assignment first resolves all destination
places, then materializes its complete RHS, then validates overlap, bounds, and
slice replacement lengths before performing any write. Compound assignment
uses an access validation before reading its previous value and validates the
fully computed replacement again before storing it. Static callees remain
callable operands instead of being erased into ordinary temporaries, preserving
the selected declaration, receiver mode, generic specialization, and variadic
argument association.

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
  callable;
- aggregate, conversion, iterator, index, slice, range, membership, and tag
  operations have the exact instantiated input and result types;
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

`Operation::Run` performs this complete lowering and verification today. Until
bytecode and VM execution are connected, a successfully verified graph reaches
the deliberate `T0001` phase marker; malformed MIR never reaches that marker.
