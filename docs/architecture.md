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
specializations close invariant inference and prove every closed intrinsic
constraint (`Copy`, `Discard`, `Equatable`, `Key`, `Send`, and `Share`) before
leaving HIR. Named free and receiver-free associated functions may cross that
boundary as uniform values only after their exact `fn(...)` type and complete
specialization are known; receiver methods never become implicit bound values.
Closure expressions cross as distinct generated types with an exact signature,
separate checked body, inherited binders, and a syntactic by-value environment.
CALL-004 preserves the four sync/unsafe/async effect combinations in both the
generated identity and function type. CALL-003 derives `Call`, `CallMut`, and
`CallOnce` from reachable capture accesses; an async environment write removes
both shared and exclusive borrowed invocation. Contextual closure-to-`fn(...)`
erasure requires an exact effect-preserving signature, `Call`, and an
environment proving `Copy + Send + Share`. The executable M4 subset still
requires Copy captures and only invokes synchronous-safe signatures; affine
capture moves remain an M5 boundary, async initiation belongs to M7, and unsafe
context validation belongs to M9. Open source/prelude trait obligations use
coherent static selection. Trait declarations
carry a sorted method table, contextual `Self`, default-body and async-receiver
requirements. Default bodies are checked once with rigid trait binders; calls to
another receiver method of the same trait resolve locally and both inferred and
explicit method generics preserve the enclosing trait arguments. Implementations
carry IDs derived from logical source order, their complete trait/target header,
generic binders, source-ordered methods, and the instantiated contract of each
method. HIR enforces determinable binders, module-based orphan rules, exact
signatures and bounds, required/default membership, and the closed/open prelude
protocol split before checking implementation bodies. A separate program-wide
coherence pass alpha-renames each implementation's binders, ignores positive
bounds, rejects unifiable complete headers, and enforces the functional
`Iterator[T]` target-to-element relation. Once coherence succeeds, size-change
termination turns every open-trait header bound into a canonical query edge,
constructs structural `<`/`=`/`?` matrices, and saturates them inside trait-name
SCCs. Every idempotent self matrix must decrease on its diagonal; otherwise HIR
emits `E1112` with a deterministic cycle witness. Closed structural capabilities
create no trait-selection edges, all analysis uses an explicit work budget, and
the admission verifier independently reconstructs the proof before MIR. Static
constraint selection and trait dispatch produce direct specialized callables.
Pattern checking is part of the
same typed-HIR boundary and records typed pattern arenas, guarded match arms,
irrefutability, reachability, and exhaustiveness without deferring decisions to MIR. Assignment
checking resolves target projections before the RHS and records compound
operators, per-leaf conversions, write extent requirements, and tuple write
order explicitly. Structured control-flow checking records normal completion
separately from contextual types, assigns loop identities, propagates `Never`,
and diagnoses unreachable evaluation boundaries with a top-down HIR worklist.
Explicit discard is distinct from assignment and carries a structural
`Discard` proof. The same coinductive symbolic nominal summaries derive all six
closed capabilities without recursively expanding nominal type families.
Intrinsic loops retain a distinct `cursor[own,C]` or `cursor[ref,C]` state type,
so capability derivation and later ownership analysis never confuse mutable
iteration state with its source collection.
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
misaligned flow metadata as compiler defects. It also re-derives implementation
signatures from their source or prelude trait, proves table/callable
correspondence and orphan ownership, and rejects incomplete contracts as compiler
defects. It also proves one-to-one closure construction metadata, generated
identity/signature effect agreement, async parameter restrictions, and exact
owned capture type, mutability, and source binding. Closure protocols, call
signatures, access selection, generic and opaque call bounds, and
callable-erasure preconditions are rederived rather than trusted as checker
annotations. An effectful ordinary call is rejected at this boundary. Partial HIR remains available to
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
six-column capability decision for every interned type and never turns `_` into
a hidden write. Type formation rejects `Map[K, V]` and `Set[K]` without `K: Key`
and `Ref[T]` without `T: Discard`; equality, membership, map lookup, opaque
bounds, and async receiver implementations consume the same proof. Full
ownership availability and terminal cleanup remain later analyses. Synchronous
Copy closure invocation already crosses this boundary with an explicit exact
signature and selected call protocol.

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
- Closure aggregates preserve the exact generated type and copy each capture
  from its declared outer source binding in HIR order.
- The ordinary call operation carries only a synchronous-safe signature;
  effectful initiation requires the later async or unsafe MIR operation.
- Capability-sensitive equality, membership, and map lookup agree with the
  independently verified HIR capability table.
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
returning a program to execution. It independently derives the closed
capabilities needed by type formation, equality, membership, and map lookup
from the concrete bytecode type graph and nominal layout templates, rather than
trusting a compiler-produced boolean.

Before those tables are allocated, a bounded deterministic worklist
monomorphizes every generic callable reached from non-generic roots, constants,
or another concrete instance. Equal callable/argument pairs share one body;
same-instance recursion terminates by deduplication and type-expanding recursion
terminates at the request limit. Executable callable signatures and function
bodies contain only concrete types and direct calls carry no runtime type pack.
Uniform function values stored in locals, aggregates, or constants use the same
verified indirect-call operation; source-trait values are statically selected
before entering the constant or operand catalog.
Concrete closure construction uses an ordinary generated-type aggregate with a
verified capture schema. Each reached closure specialization also receives one
real callable and function body with a hidden environment parameter. Calls use
the ordinary verified indirect-call operation, carrying an exact signature and
concrete protocol; a shallow environment borrow is confined to the immediate
callee position while `CallOnce` retains ordinary copy/move operand semantics.
All four closure effect signatures survive in the callable catalog, but the
ordinary call operation and bytecode verifier reject `async` or `unsafe`
signatures until their effect-aware instructions exist.
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
normal/unwind continuations, precise frame and temporary roots, generational
heap handles, and a stop-the-world mark-and-sweep collector. Closure
environments trace, snapshot, and copy their capture fields through the same
managed-value machinery regardless of their effect signature. The VM rejects
effectful ordinary calls and effectful root entries, so retaining an async or
unsafe callable cannot activate an unfinished runtime. Its exact object,
tracing, panic, host, and admission boundary is recorded in
`docs/contracts/vm-runtime.md`.
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
