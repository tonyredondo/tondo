# Typed slot bytecode contract

**Status:** BC-001 through BC-005 implemented; VM execution pending

This document fixes the in-memory boundary between `tondo-compiler` and
`tondo-vm`. It is an implementation contract, not observable Tondo syntax or a
stable artifact format. `TONDO_LANGUAGE_SPEC.md` remains normative.

## Ownership and admission

`tondo-vm` owns the bytecode data model, verifier, and future interpreter.
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

Callable metadata retains receiver position, parameter modes, variadic element
type, generic arity, outcome, and function type. Static function operands name
that callable plus explicit type arguments. Indirect calls retain a structural
function type. Named constants are already evaluated and normalized; execution
never invokes arbitrary code to initialize them.

## Function tables and slots

Each function owns:

- a strictly ordered set of global type IDs used by that body;
- a sorted, deduplicated source-span table;
- typed frame slots for the return place, parameters, user locals, and
  temporaries;
- parameter, entry, unwind, and return-slot indices; and
- basic blocks in deterministic MIR order.

Every executable item references a function-local span-table index. All spans
remain in the function's source file and use semi-open byte ranges. The
function source span is retained separately for symbolication and diagnostics.

Slots are explicit roots. There is no operand stack whose types or liveness
must be reconstructed at an instruction offset. `StorageLive` and
`StorageDead` reserve the later ownership/cleanup boundary; parameters and the
return place have function-wide storage.

## Instructions and control flow

Ordinary instructions perform storage lifetime changes or one typed store from
a pure rvalue. Rvalues cover loads, copies/moves, constants, pure arithmetic,
construction, record update, coercion, total conversion, range, membership,
length, and iterator-state creation.

Potentially panicking work remains a terminator-level `Invoke` with explicit
normal destination/target and cleanup target. This includes checked arithmetic,
map construction, indexing, slicing, calls, `assert`, and `panic`. Other
terminators cover direct branches, boolean and discriminant dispatch,
iterator-next, atomic destination validation, return, panic resumption, and
unreachable code.

Places start at one slot and carry typed projections. Projections include
record/newtype fields, tuple positions, enum/option/result/union payloads,
array-pattern segments, dynamic indexing, and slices. Index and bound operands
are slots evaluated earlier, preserving MIR evaluation order.

## Independent verification

Before execution, the verifier proves:

- every type, nominal, callable, constant, function, slot, span, block, and
  pool index exists;
- catalogs, local type tables, span tables, implementations, and parameter
  tables are unique and internally linked;
- type constructors, generic arities, nominal fields/variants, constants,
  projections, aggregates, operators, conversions, iterators, and tags have
  their exact structural types;
- calls have a function callee, matching outcome, complete fixed/receiver
  association, correct modes, and valid variadic element or final spread;
- normal edges remain in normal code, unwind edges enter cleanup code, and the
  distinguished unwind block resumes panic;
- all reachable reads have a dominating live definition, edge-produced values
  exist only on their successful edge, and the return slot is initialized;
- payload projections are dominated by their matching discriminant edge and a
  potentially overlapping write invalidates that refinement; and
- unreachable retained blocks contain no executable bytecode.

Initialization/lifetime and discriminant refinement are separate forward
dataflow analyses with an explicit shared step budget. Exhaustion is a resource
limit, not malformed source and not permission to execute partially verified
code.

## Determinism, limits, and tooling

Catalogs follow stable HIR/MIR order; type-use sets and span tables are sorted.
No observable ordering depends on hash iteration. Construction bounds types,
nominals, callables, constants, functions, per-function slots, blocks,
instructions, and spans. Driver exhaustion becomes `T0002`.

`disassemble` renders deterministic human-readable text for tests and debugging
and labels itself tooling-only. There is no bytecode serializer or loader in
the bootstrap. The text, enum layout, dense indices, and Rust representation
may change without compatibility guarantees.
