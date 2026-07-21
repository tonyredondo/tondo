# Typed slot bytecode contract

**Status:** BC-001 through BC-005, GEN-002 monomorphization, and the M3 VM
admission path implemented

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

The VM data model can retain receiver position, parameter modes, variadic
element type, generic arity, outcome, and function type. Compiler-produced
executable callable entries are concrete instances: their generic arity is zero
and their signature types have already been substituted. Static function
operands name that concrete callable and carry an empty type-argument vector.
Indirect calls retain a structural concrete function type. Named constants are
already evaluated and normalized; execution never invokes arbitrary code to
initialize them.

## Monomorphization boundary

`lower_to_bytecode` discovers concrete callable instances before allocating any
bytecode table. It roots every non-generic callable and every specialized
function value reachable from an evaluated constant, then transitively scans
the reached MIR templates for static function operands. Nested type arguments
are substituted with the enclosing instance before their callee is queued.
Trait defaults retain a hidden generic `Self` position, even on otherwise
non-generic traits, so declaring a default never makes it an executable root.
Static dispatch must select and specialize that template before it can enter the
worklist. A concrete non-generic implementation method already has an ordinary
checked HIR/MIR body and may enter the bootstrap worklist under its stable
`implementation#N.method#M` identity; generic implementation methods remain
templates until dispatch supplies their header arguments.

Instances are deduplicated by callable identity plus the complete concrete
argument vector. Direct recursion with the same vector therefore terminates.
Type-expanding recursion creates distinct instances and stops with `T0002` when
the request's generic-instantiation budget is exhausted. The same failure rule
applies if substitution exhausts the interned specialized-type budget. No
partial program crosses the verifier boundary.

For each reached function, lowering builds a complete template-to-concrete map
covering its signature, locals, places, projections, operands, rvalues,
operations, discriminant tags, and outcome. A missing mapping or a surviving
generic/inference node is an internal construction error. Unreferenced generic
functions have no bytecode body. Equal specializations reached from several
calls or constants share one callable and one function entry.

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
unreachable code. A write validation aligns each destination with an optional
copied replacement; a slice write must include an `Array` replacement of the
place type, allowing the VM to raise `P0006` before the first store. Missing or
misaligned metadata is invalid bytecode.

Places start at one slot and carry typed projections. Projections include
record/newtype fields, tuple positions, enum/option/result/union payloads,
array-pattern segments, dynamic indexing, and slices. Index and bound operands
are slots evaluated earlier, preserving MIR evaluation order.

Map construction includes an explicit reject-versus-replace flag for dynamic
duplicate keys. The VM evaluates the already-materialized entry operands in
order, detects duplicates before allocating the final map, and either preserves
the first insertion position while replacing its value or raises `P0009`.

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
- every `assert` retains a nonempty condition representation for its default
  runtime message; and
- unreachable retained blocks contain no executable bytecode.

Initialization/lifetime and discriminant refinement are separate forward
dataflow analyses with an explicit shared step budget. Exhaustion is a resource
limit, not malformed source and not permission to execute partially verified
code.

## Determinism, limits, and tooling

Catalogs follow stable HIR/MIR order; instance sets, type-use sets, and span
tables are sorted. No observable ordering depends on hash iteration.
Construction bounds generic instances, specialized and catalog types, nominals,
callables, constants, functions, per-function slots, blocks, instructions, and
spans. Driver exhaustion becomes `T0002`.

`disassemble` renders deterministic human-readable text for tests and debugging
and labels itself tooling-only. There is no bytecode serializer or loader in
the bootstrap. The text, enum layout, dense indices, and Rust representation
may change without compatibility guarantees.
