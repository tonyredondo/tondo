# Tondo compiler architecture

**Status:** bootstrap baseline  
**Language baseline:** Tondo 0.1-draft.8  
**Implementation version:** 0.0.0

This document defines implementation boundaries and phase invariants. It is not
a source-language specification. If this document and the language
specification disagree about observable Tondo behavior, the language
specification wins.

## Objectives

The compiler architecture must:

- Preserve a single interpretation of every accepted program.
- Keep source, diagnostics, formatting, and semantic tooling on one frontend.
- Make phase boundaries explicit enough to test independently.
- Preserve stable logical identities and byte spans through every phase.
- Support ownership, cleanup, and async lowering without redesigning the IR.
- Produce deterministic output from identical declared inputs.
- Reach an executable bytecode vertical slice before adding a native backend.

The bootstrap does not optimize for incremental compilation, compact runtime
representation, native code generation, or parallel execution.

## Workspace boundaries

The workspace begins with three crates:

- `tondo-cli`: process arguments, filesystem ingress, stream routing, and exit
  codes. It contains no language semantics.
- `tondo-compiler`: source database, syntax, diagnostics, name resolution,
  semantic analysis, HIR, MIR, and bytecode generation.
- `tondo-vm`: bytecode verification and execution.

Logical compiler modules remain modules inside `tondo-compiler` until a stable
ownership or dependency boundary justifies another crate. Crate boundaries are
not used as a substitute for module design.

## Public compilation path

All entry points construct one `CompilationRequest` and call one driver:

~~~text
CLI or embedding host
  -> CompilationRequest
  -> source validation
  -> lossless CST
  -> canonical formatter (format operation)
  -> resolved HIR
  -> typed HIR
  -> MIR
  -> verified bytecode
  -> VM
  -> CompilationOutput
~~~

The request carries every build input that may affect results:

- Operation: format, check, or run.
- Language edition.
- Target and host profile.
- Declared target capabilities.
- Diagnostic format.
- Source form: module, script, or fragment.
- Resource limits.
- Closed package graph with exact package, standard-library, dependency, and
  module identities.
- Closed source database and root file.

No phase reads process environment, current directory, network, locale, wall
clock, or random state. The CLI may read a physical file to construct the
request; physical paths do not become language identities.

## Source model

`SourceDatabase` owns immutable byte snapshots. A source may be invalid UTF-8;
the lexer must diagnose that without losing the original byte offset.

Each file has:

- An opaque `SourceId` identifying its logical owner.
- A canonical NFC module path.
- A canonical relative logical file path using `/`.
- A physical or virtual origin marker that is not semantically observable.
- Immutable bytes shared through `Arc<[u8]>`.
- A lazily constructed line index.

`FileId` is local to one request and is never serialized. Diagnostics resolve it
back to the stable source ID, module, path, and byte range before leaving the
compiler.

## Phase invariants

### Source validation

Input bytes are immutable. Logical paths are canonical. Duplicate logical
source keys are rejected. Every range is semi-open and validated against its
file.

### Syntax

The lexer emits tokens and trivia with byte ranges. The parser creates a
lossless CST, including comments, whitespace, unexpected tokens, and recovery
nodes. It does not ask the type system to decide syntax. Contextual forms remain
preliminary nodes until name resolution has enough information to classify
them.

Every physical byte is owned by exactly one non-synthetic token. Logical `NL`,
`EOF`, and missing recovery tokens are zero-width synthetic tokens, so newline
classification and error recovery never duplicate or discard source bytes. The
exact CST contract is recorded in ADR-003; the implemented lexer, parser,
recovery, and typed-view boundaries are recorded in
`docs/contracts/syntax.md`.

The typed AST is a view over the CST. It does not copy source strings or create
a competing syntax tree.

The formatter consumes the same accepted `Parsed` CST, including trivia and
comments. It never formats a tree carrying lexical or syntax diagnostics. Its
fixed-width document renderer produces canonical UTF-8 bytes independently of
the host platform; the exact boundary is recorded in
`docs/contracts/formatter.md`.

### Resolution and HIR

Resolution assigns every declaration and reference a stable semantic ID inside
the request. HIR removes purely syntactic distinctions while retaining source
origins. Every name in resolved HIR is either bound or carries a primary
diagnostic; later phases do not perform fallback name lookup.

The build-input and nominal-identity boundary is recorded in
`docs/contracts/package-graph.md`. Declaration collection, lexical scopes,
member tables, contextual names, visibility, API validation, recovery, and
determinism are recorded in `docs/contracts/resolution.md`.

Semantic type lowering then expands aliases, normalizes source type spellings,
materializes nominal declarations and callable signatures, and validates
recursive productivity. Expression checking then materializes typed constants,
bootstrap callable bodies, nominal constructors and updates, closed operators,
calls with declaration-bound arguments, and explicit generic specializations
with resolved identities, value categories, and contextual coercions. Generic
specializations close invariant inference and prove the structural `Discard`
constraint before leaving HIR; other capability and trait obligations remain
explicit incomplete boundaries until their owning M4 phases. Trait declarations
carry a sorted method table, contextual `Self`, default-body and async-receiver
requirements. Default bodies are checked once with rigid trait binders; calls to
another receiver method of the same trait resolve locally and both inferred and
explicit method generics preserve the enclosing trait arguments. `impl`,
coherence, and trait dispatch remain separate later phases. Pattern checking is
part of the same typed-HIR
boundary and records typed pattern arenas, guarded match arms, irrefutability,
reachability, and exhaustiveness without deferring decisions to MIR. Assignment
checking resolves target projections before the RHS and records compound
operators, per-leaf conversions, write extent requirements, and tuple write
order explicitly. Structured control-flow checking records normal completion
separately from contextual types, assigns loop identities, propagates `Never`,
and diagnoses unreachable evaluation boundaries with a top-down HIR worklist.
Explicit discard is distinct from assignment and carries a structural
`Discard` proof; symbolic nominal summaries avoid recursive type expansion.
Constants are then evaluated from typed HIR by a closed, non-executing
worklist. Dependency SCCs and their topological order use stable symbol
identities; normalized values remain in HIR for later MIR/bytecode lowering,
while compile-time panics, nonconstant work, duplicate collection entries, and
known NaN comparisons receive their normative diagnostics.
The driver retains those facts in an immutable semantic snapshot. It provides
structured type, entity, reference, member, signature, and closed-call-error
queries without making tooling re-resolve the CST; partial snapshots have an
explicit phase/completion boundary.

Its exact implemented boundary, including recovery, resource limits, and
source-less external identities, is recorded in
`docs/contracts/hir.md`; the public query contract is recorded in
`docs/contracts/semantic-queries.md`.

Complete error-free HIR passes an internal admission verifier before either a
successful check or MIR lowering. It rejects recovery/inference types, dangling
or cyclic arena edges, unresolved semantic IDs, invalid value categories, and
misaligned flow metadata as compiler defects. Partial HIR remains available to
semantic tooling but is never executable. The phase ownership of moves, loans,
cleanup, and suspension is fixed by ADR-016 and `docs/contracts/mir.md`.

### Typed HIR

Every expression has exactly one static type. Aliases are expanded where the
spec requires canonical comparison; nominal IDs remain distinct. Inference
variables do not cross a completed function body or public signature boundary.

Fallible callables retain both the logical success expectation and the complete
`Result` type. Success lifting, union injection/widening, option lifting, and
diverging conversions are explicit HIR nodes. MIR therefore never has to infer
which contextual representation was selected by type checking.

Every expression also carries a `MayComplete` or `Diverges` summary. Loop-local
break targets are resolved before MIR, and unreachable-code warnings follow the
same explicit evaluation order retained for assignments and calls. MIR may
lower these facts into edges; it must not reinterpret source reachability.

Standalone discard and discard leaves are also explicit. MIR receives a
completed capability decision for the supported concrete type subset and never
turns `_` into a hidden write. Full ownership availability and terminal cleanup
remain later analyses.

Type IDs are request-local interned handles; only canonical recursive type
strings are observable. Alias expansion, union normalization, nominal identity,
assignment, local inference algorithms, and the type-node resource boundary are
recorded in `docs/contracts/types.md`.

### MIR

MIR is a typed control-flow graph with explicit locals, temporaries, branches,
moves, storage lifetimes, checked-operation unwind edges, and reserved cleanup
blocks. AST shape is no longer required to execute or analyze the program.
Ownership later adds loans and populated cleanup actions; async later adds
suspension, resume, cancellation, and frame-state edges without moving source
semantic decisions into a backend.

Before bytecode lowering, the MIR verifier proves:

- Every block terminates correctly.
- Every operand and destination has a compatible type.
- Every use is dominated by an available definition.
- Cleanup edges are well formed.
- Payload projections are dominated by a compatible discriminant branch.
- Calls preserve the selected callable, receiver mode, specialization, and
  argument association.
- No unresolved inference, symbol, or contextual syntax node remains.

The current M3 lowering covers the complete error-free bootstrap HIR surface.
`tondo run` always lowers and verifies MIR before entering bytecode generation;
construction and verifier work are bounded by the request's explicit resource
limits.

### Bytecode

Bytecode uses explicit frame slots rather than an implicit operand stack. It
retains function type information, source spans, and root metadata. The VM
verifies all bytecode before execution, including bytecode produced by the
reference compiler.

The implemented M3 format has one canonical program type/nominal/callable/
constant catalog and a closed type-use and span table per function. Instructions
read and write typed places over slots; terminators preserve ordinary, cleanup,
unwind, iterator, discriminant, and return edges. Lowering is deterministic and
the VM-owned verifier independently rechecks indices, instantiated layouts,
calls, initialization, storage lifetime, tag refinement, and edge shape before
returning a program to execution.

Before those tables are allocated, a bounded deterministic worklist
monomorphizes every generic callable reached from non-generic roots, constants,
or another concrete instance. Equal callable/argument pairs share one body;
same-instance recursion terminates by deduplication and type-expanding recursion
terminates at the request limit. Executable callable signatures and function
bodies contain only concrete types and direct calls carry no runtime type pack.
Generic nominal declarations remain compact layout templates checked with their
concrete arguments by the verifier.

The bootstrap bytecode exists in memory and is not a stable artifact or ABI.
Its disassembler is tooling text only and there is deliberately no loader.

### VM

The VM starts with explicit Rust enums for values, a precise non-moving tracing
heap, and a cooperative single-thread executor. Logical value copies may be
eager. COW, ARC, compact tagging, and native lowering are later optimizations
that must preserve the same tests.

The implemented synchronous engine uses iterative frames, checked slot states,
normal/unwind continuations, precise roots, generational heap handles, and a
stop-the-world mark-and-sweep collector. Its exact object, tracing, panic, host,
and admission boundary is recorded in `docs/contracts/vm-runtime.md`.
The sole M3 standard-library bridge, capability-gated
`std.console.print(String): Unit`, is isolated by
`docs/contracts/bootstrap-host.md` and is not a general FFI or a frozen stdlib
ABI.

## Data ownership across phases

- `SourceDatabase` owns source bytes for the full compilation request.
- CST nodes own indices and ranges, never borrowed slices with fragile
  lifetimes.
- HIR and MIR use request-owned arenas and stable IDs. Bytecode converts those
  identities to dense request-local catalogs owned by the execution request.
- Each phase consumes or immutably observes the previous phase; it does not
  mutate source or reinterpret prior diagnostics.
- `CompilationOutput` owns resolved diagnostics and command stdout, including
  canonical formatter bytes, plus later produced artifacts. After resolution
  it may also own a `SemanticModel` containing the exact source database,
  resolved program, and available typed HIR. It never borrows the request.
- VM roots are explicit in frames, environments, host handles, and suspended
  task state.

This model avoids self-referential Rust structures and lets a phase be tested
from immutable snapshots.

## Diagnostics

Diagnostics are structured values first. Human text and JSON Lines are renderers
over the same report. A diagnostic keeps `Span` values internally and resolves
them only when the report is finalized.

The report:

- Computes normative SHA-256 IDs.
- Sorts primary diagnostics using logical identities and byte ranges.
- Sorts and deduplicates related locations, fixes, and edits.
- Merges repeated diagnostics with the same normative ID.
- Emits all mandatory JSON keys, including explicit `null` location fields.

Implementation diagnostics use a prefix outside the normative `E`, `W`, and `P`
namespaces. `T0001` means that the requested bootstrap pipeline has no
implementation yet; `T0002` reports an explicit implementation resource limit.

## Failure boundaries

- Invalid Tondo source produces diagnostics and exit code 1.
- Invalid CLI usage or unreadable CLI input produces exit code 2.
- An internal toolchain failure produces exit code 3.
- Panics represent compiler defects, not user diagnostics.
- The driver returns typed errors to embedding callers; only `tondo-cli` decides
  process streams and exit codes.

## Determinism

Observable ordering never depends on Rust hash-map iteration. Stable output uses
canonical strings, ordered maps/sets, explicit sorting, and declared request
inputs. Physical paths stay at the CLI boundary.

Incremental compilation may later cache phase results, but a cache hit and a
clean build must produce identical outputs.

## Validation strategy

Every phase has:

- Unit tests for local algorithms and invariants.
- Inline virtual-source tests for integration without filesystem identity.
- Compile-pass and compile-fail fixtures once language phases exist.
- Human and JSON golden output.
- Runtime fixtures once bytecode execution exists.
- Deterministic arbitrary-byte and grammar-generated corpora before persistent
  fuzz targets are added for the lexer, parser, formatter, bytecode loader, and
  JSON protocol.

The conformance suite remains independent from implementation-specific tests.

## Change rule

An architectural change must:

1. State which invariant or measured constraint requires it.
2. Update or supersede the relevant ADR.
3. Preserve observable language behavior or identify the required spec change.
4. Add tests at the boundary where the old design failed.
