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
environment proving `Copy + Send + Share`. OWN-006 applies the same contextual
Copy/Move decision as an ordinary value transfer to every capture, marks an
affine outer binding unavailable at construction, and derives repeatable call
protocols from environment writes and moves. OWN-007 derives `CallOnce` only
when every capture is `Discard` or leaves its environment slot on every normal,
return, failure, and propagation exit. Ordinary invocation still accepts only
synchronous-safe signatures; async initiation belongs to M7, and unsafe context
validation belongs to M9. Open source/prelude trait obligations use coherent
static selection. Trait declarations
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
Constant and runtime array indexing share the bytecode boundary's single
overflow-free positive/negative index normalizer.
Constant and runtime array slicing likewise share one normalizer for optional
start/end/step operands, including direction-dependent defaults, clipping,
`Int.min`, and the zero-step panic.
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
bounds, and async receiver implementations consume the same proof. HIR now
adds a deterministic flow proof that an owned binding is available on every
path reaching a use. A plain assignment to a direct `var` may create a new
definition after its RHS completes; every other target still requires an
available root. HIR also records and rederives one uniform
copy/observe/consume mode per `match`, rejects unconfirmed affine projections,
and delays affine pattern transfers until after a successful guard. Typed move
paths now live in MIR and bytecode as an internal destructuring mechanism;
closure captures participate in the same whole-owner flow and become typed
environment move paths. Closure terminal obligations are intersected across all
normal exits. HIR now also reserves fixed `ref`/`mut`/`var` call arguments in
source order and rejects overlapping later argument access. Last-use and
collection loan regions and confirmed borrowed transfers are explicit. A
separate closed registry classifies direct terminal roots and derives structural
`Absent`/`Potential`/`Present` status without treating every non-`Discard` value
as the same resource. The availability state follows every non-absent terminal
owner through bindings, compounds, closures, consuming patterns, calls,
assignments, observed temporaries, loops, and control transfers. A normal path
may leave a scope only after a visible consumption or confirmed handoff; panic
paths deliberately keep the closed fallback armed. Synchronous closure
invocation crosses this boundary with an explicit exact signature and selected
call protocol. Every lexical block has a stable cleanup-scope identity. Checked
`defer` actions capture `Copy` operands at registration and reserve at most one
complete affine owner as an explicit guard. In HIR that owner is a local or
temporary; MIR and bytecode may represent the same owner as a local slot or one
closure-capture slot. The availability proof rejects
duplicate guards, partial moves, embedding and overwrite, follows a permitted
whole-owner move, and disarms only after a confirmed handoff or intrinsic
natural exhaustion. TERM-004 materializes closed fallbacks at owning entry
slots and every terminal result edge, specializes generic registrations to
concrete presence, and drains them with explicit entries through one abnormal
LIFO ledger. Structural fallback traversal handles all current aggregate
layouts; M7 supplies the suspendable task state needed by direct
`JoinTeardown`.

Type IDs are request-local interned handles; only canonical recursive type
strings are observable. Alias expansion, union normalization, nominal identity,
assignment, local inference algorithms, and the type-node resource boundary are
recorded in `docs/contracts/types.md`.

### MIR

MIR is a typed control-flow graph with explicit locals, temporaries, branches,
moves, storage lifetimes, checked-operation unwind edges, and reserved cleanup
blocks. AST shape is no longer required to execute or analyze the program.
Array aggregates carry one operand per runtime element while their canonical
`Array[T]` type contains no length; array-pattern shape checks use the typed
`Length(Array[T]) : Int` rvalue.
OWN-002 selects explicit copy/move operands under each body's generic bounds
and uses non-escaping borrows for immediate observations. BORROW-001 gives
non-value call arguments explicit loan-table identities, ordered reservations,
call consumption, and release on abandoned argument paths. OWN-003 first made
each root move consume the local's available
definition and joined that fact across CFG edges and loop backedges. OWN-004
uses the existing unprojected write as the new definition for a reinitialized
`var`, only after RHS evaluation and validation. OWN-005 replaces the backend
whole-local bit with typed unavailable paths: disjoint destructured components
may move independently, overlapping paths cannot be reused, writes restore
only a proved subtree, and joins conservatively union moved paths. The loan
verifier propagates exact active sets across that same CFG, rejects incompatible
fixed-place overlap and illegal reborrows, and confines each loan operand to its
call. Last-use pattern regions, static collection-region disjunction, and
runtime-dependent overlap checks use the same paths. TERM-003 adds
`RegisterDefer`, `RetargetCleanup`, `DisarmCleanup`, and scope-specific
`DrainDefers` operations plus a second exact dataflow ledger for registrations
and affine guards. TERM-004 extends it with `RegisterFallback` and one
`DrainUnwind`: normal completion drains only explicit scope entries and drops
abnormal markers after the frontend proof, while panic drains both kinds in
unified LIFO order. TERM-005 requires each terminal explicit guard to replace
exactly one fallback atomically and forbids rearming either representation
across the other. Retarget and disarm therefore move the single obligation,
never a pair. An owning intrinsic iterator carries an edge-specific exhaustion
guard when its collection may be terminal, so only the exhausted edge disarms
it. Monomorphization removes that marker and nonterminal fallback registrations
for a closed specialization. Async later adds suspension, resume, cancellation,
active `Join` state, and frame-state edges without moving source semantic
decisions into a backend.

Before bytecode lowering, the MIR verifier proves:

- Every block terminates correctly.
- Every operand and destination has a compatible type.
- Every use is dominated by an available typed path and each root or projected
  move consumes that path exactly once per CFG route.
- Cleanup edges are well formed.
- Payload projections are dominated by a compatible discriminant branch.
- Calls preserve the selected callable, receiver mode, specialization, and
  argument association.
- Every non-value call argument consumes one matching active loan; reservations
  agree at joins, reject overlapping exclusive access, and are explicitly
  released on abandoned normal paths.
- Closure aggregates preserve the exact generated type and contextually copy or
  move each capture from its declared outer source binding in HIR order.
- Closure bodies rederive their protocol row from writes and moves of the exact
  hidden-environment paths.
- The ordinary call operation carries only a synchronous-safe signature;
  effectful initiation requires the later async or unsafe MIR operation.
- Capability-sensitive equality, membership, and map lookup agree with the
  independently verified HIR capability table.
- Defer registrations form one exact scope-nested LIFO ledger, every affine
  guard transition is backed by the immediately preceding complete move, and
  no active entry can cross an undrained return or panic-resume edge.
- An owning intrinsic iterator has an exhaustion guard exactly when its
  contextual collection can contain a terminal token, and that guard names the
  cursor's exact internal source path.
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
trusting a compiler-produced boolean. It likewise rederives terminal presence
from a sealed `Join` contract and the concrete ownership graph, rejecting an
opaque witness that would hide a terminal token. Its independent defer ledger
rechecks scope nesting, unique registration, complete affine guards, immediate
retarget/disarm transitions, exact terminal fallback replacement, exclusion
between explicit and fallback entries, and exact draining. Closed
specialization also rederives whether an intrinsic owning cursor needs its
edge-specific natural exhaustion disarm.
Array construction stores operand count in the value, never in the type
catalog; the verifier seals the internal `Length` operation to
`Length(Array[T]) : Int`.
Array indexing likewise retains one signed `Int` operand, while the verifier
seals both value operations and place projections to `Array[T]`.
Array slicing retains three optional `Int` operands without inventing sentinels;
the verifier seals its operation and projections to `Array[T]`, and the VM
normalizes the exact operands only after the runtime length is known.

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
verified capture schema and explicit Copy/Move operands. Each reached closure
specialization also receives one real callable and function body with a hidden
environment parameter. Calls use the ordinary verified indirect-call operation,
carrying an exact signature and concrete protocol; a shallow borrow is confined
to verified immediate observation and indirect-callee positions, while
non-value arguments carry verified call-local loan identities and `CallOnce`
retains ordinary copy/move operand semantics. Bytecode rederives environment
writes and moves before accepting that protocol row, and accepts `CallOnce` only when each
non-`Discard` capture is completely transferred on every reachable return.
Monomorphization specializes the source-generic row against concrete `Discard`
facts before that independent verification.
All four closure effect signatures survive in the callable catalog, but the
ordinary call operation and bytecode verifier reject `async` or `unsafe`
signatures until their effect-aware instructions exist.
Generic nominal declarations remain compact layout templates checked with their
concrete arguments by the verifier.

The bootstrap bytecode exists in memory and is not a stable artifact or ABI.
Its disassembler is tooling text only and there is deliberately no loader.

### VM

The VM starts with explicit Rust enums for values, a precise non-moving tracing
heap, and a cooperative single-thread executor. Logical copies of every
compound `Copy` shape are currently eager and recursive; immutable String
storage and `Ref[T]` identity are the only deliberate sharing cases. COW, ARC,
compact tagging, and native lowering are later optimizations that must preserve
the same observables. The `tests/runtime/value-copy/` corpus records those
observables at the driver boundary and runs unchanged both with ordinary
limits and with an initial GC threshold of one, without exposing the current
heap representation.
An Array heap object owns its ordered runtime slots; construction, copy,
argument passing, and return preserve that vector length independently of the
single canonical `Array[T]` type.
Every read, write, borrowed place, and constant array access uses the same
normalization rule; invalid endpoints unwind as `P0001`.
Every slice read, projected write, loan path, overlap proof, and constant slice
uses the same direction-aware normalization rule; a zero step unwinds as
`P0002`.

The implemented synchronous engine uses iterative frames, checked slot states,
normal/unwind continuations, call-local reservation tables, normalized
frame/slot/place identities, precise frame and temporary roots, generational
heap handles, and a stop-the-world mark-and-sweep collector. Borrowed callee
parameters read and write through to the lender place, and early control or
panic releases abandoned reservations. Closure
environments trace, snapshot, copy eligible fields, and take moved fields
through the same managed-value machinery regardless of their effect signature.
The VM derives a sealed trace catalog independently from verified bytecode:
each heap allocation retains its type descriptor, each mutation is shape
checked before publication, and each function has an exact slot-schema
descriptor reusable by a future suspended frame. Opaque results reuse their
witness layout; closure environments retain their unique callable and capture
schema. `Ref[T]` uses that traceable cell shape for source-visible
`Ref(value)` construction. Copying it preserves one handle identity,
`.value` remains a shared read-only projection, and equality plus collection
keys compare the identity rather than its payload.
The operation-local root stack protects completed children and operands across
every later allocation, including constants, compound host returns, recursive
copies, projections, slices, array arithmetic, variadic packing, calls, and the
structured terminal-fallback walker. Publication into a frame, cleanup, or
managed object and withdrawal by move/death/pop have explicit transitions.
Closure environments remain ordinary traced objects. Host values are detached
snapshots rather than handles; suspended-frame containers remain absent until
M7 registers them as a new root source. The VM rejects effectful ordinary calls
and effectful root entries, so retaining an async or unsafe callable cannot
activate an unfinished runtime. A test-only memory adapter drives the same
allocator, descriptors, root enumeration, and pressure trigger to keep a mixed
`Ref`/array/closure cycle alive, repeatedly reclaim unrooted peers, and reclaim
the retained graph after root withdrawal. Source-level REF-001 construction
reuses that same cell and collector path; the adapter remains necessary only
for cyclic graphs that safe read-only `Ref` values cannot construct directly.
Its exact capacity gate also unifies object and byte limits: a request performs
at most one full collection, rechecks capacity, and publishes once.
Replacement roots its target internally and remains unchanged if the retry
cannot fit. The exact object, tracing, panic, host, and admission boundary is
recorded in `docs/contracts/vm-runtime.md`.
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
- Current VM roots are explicit in active frames and cleanups, the
  operation-local root stack, pending publications, and managed-object edges.
  Host values are detached and no suspended task state exists yet; any future
  handle or suspended-frame container must register a new explicit root source.

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
