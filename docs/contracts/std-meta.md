# `std.meta` companion contract

`std.meta` is the only public API that a provider or manifest generator uses to
consume a meta snapshot and return generated source. The 0.1 API is identified
by `tondo-std-meta-0.1/1` and is deliberately value-oriented: a request owns its
snapshot, inputs, output declarations and finite limits; a response owns its
generated sources.

## Public callable signatures

These are the ordinary Tondo companion signatures; record and enum fields are
defined by the model and request contracts below.

```text
pub fn TypeRef.identity(self): String
pub fn GenerateRequest.snapshot(self): Snapshot
pub fn GenerateRequest.inputs(self): Array[Input]
pub fn GenerateRequest.outputs(self): Array[OutputSpec]
pub fn GenerateRequest.limits(self): Limits
pub fn GenerateRequest.input(self, name: String): Input ! Error
pub fn DeriveRequest.snapshot(self): Snapshot
pub fn DeriveRequest.target(self): String
pub fn DeriveRequest.module(self): String
pub fn DeriveRequest.traitIdentity(self): String
pub fn DeriveRequest.bounds(self): Array[String]
pub fn DeriveRequest.span(self): Span
pub fn DeriveRequest.limits(self): Limits
pub fn GenerateRequest.sourceBuilder(self): SourceBuilder
pub fn DeriveRequest.sourceBuilder(self): SourceBuilder
pub fn SourceBuilder.outputs(self): Array[OutputSpec]
pub fn SourceBuilder.renderType(mut self, path: String, ty: TypeRef): String ! Error
pub fn SourceBuilder.add(mut self, path: String, source: String): Unit ! Error
pub fn SourceBuilder.finish(self): GenerateResponse ! Error
pub fn SourceBuilder.finishDerive(self): DeriveResponse ! Error
pub fn stringLiteral(value: String): String
pub fn indentation(level: UInt32): String ! Error
pub fn api(): String
pub fn target(): String
pub fn profile(): String
```

## Request

`MetaRequest` contains:

- one canonical `MetaSnapshot` (`tondo-meta-model-0.1/1`);
- a sorted, duplicate-free list of named byte inputs, each hashed with
  SHA-256;
- a sorted, duplicate-free list of exact output `(path, module)` declarations;
  and
- positive `steps`, `memory_bytes` and `output_bytes` limits.

The request has no callback, capability, filesystem, environment, process,
clock, entropy, thread, suspension, FFI, unsafe or host-identity field. There is no
ambient lookup. Inputs are the only non-model values visible to the companion.

## Source builder and ownership

`MetaRequest::into_source_builder` consumes the request. The builder accepts a
source only when its path and module exactly match one declaration, the path is
relative, slash-separated and ends in `.to`, and the bytes are valid UTF-8. It
owns the accepted bytes, computes their hash, rejects duplicates and enforces
the aggregate output limit. `finish` consumes the builder and succeeds only if
every declared output appears exactly once. Outputs are returned sorted by
logical path; no partial response is published.

## Errors and determinism

Invalid limits/text/paths, duplicate or undeclared inputs/outputs, module drift,
invalid UTF-8, missing outputs and output-budget exhaustion are closed typed
errors. The caller cannot recover by reading ambient state. The snapshot and
all collections are already canonical before the companion runs, so equivalent
requests have one traversal and one output order.

The implementation lives in `crates/tondo-compiler/src/meta.rs`; it is a pure
contract layer and does not execute a provider. Execution and sandbox admission
belong to `META-VM-001`.

Ordinary source compilation now has a separate, bounded substrate in
`meta_compile.rs`: `MetaVmArtifact::compile` checks a closed `tondo-meta/meta`
package graph, selects one public concrete callable, and lowers its referenced
functions, generic instances and closures. Nested derive requests are rejected
before expansion. `MetaVmProgram::run_with_request` admits one owned request
against the actual parameter type and the invocation's memory budget, then
executes it in a fresh VM with a rejecting host. Focused tests exercise source
compilation, structured input, fresh state, malformed inputs and resource
limits.

The candidate source in `stdlib/meta/src/meta.to` now defines the public
companion records. `MetaVmArtifact::compile_entry` installs that exact immutable
package under `std.meta` for a meta compilation and validates one of the two
entry signatures. `SourceMetaProvider` executes ordinary Tondo through the
generator and derive plan runners. Internal serialization providers retain
their separate compiler-owned artifact interface.

`GenerateRequest` has private fields and the methods `snapshot(): Snapshot`,
`inputs(): Array[Input]`, `outputs(): Array[OutputSpec]`, `limits(): Limits`,
`input(name: String): Input ! Error`, and `sourceBuilder(): SourceBuilder`.
`DeriveRequest` has private fields and the methods `snapshot(): Snapshot`,
`target(): String`, `module(): String`, `traitIdentity(): String`,
`bounds(): Array[String]`, `span(): Span`, `limits(): Limits`, and
`sourceBuilder(): SourceBuilder`.

`Snapshot` exposes the canonical model as ordinary immutable records and closed
enums. Named inputs expose `name`, `bytes: Array[Byte]` and `hash`, with no
physical path. `SourceBuilder.add(mut self, path: String, source: String)`
returns `Unit ! Error`; `finish(self)` returns `GenerateResponse ! Error`.
It rejects undeclared, duplicate and missing paths, and counts UTF-8 bytes
before admitting an output. The plan validates the complete response again
before publishing any source.

`SourceBuilder.outputs(): Array[OutputSpec]` exposes its exact output slots.
A derive builder has one slot, `derive/result.to`, in `request.module()`; this
logical slot is replaced by the invocation-derived publication path. Its
`finishDerive(): DeriveResponse ! Error` requires that source and returns empty
diagnostics and mappings. Calling `finish` on a derive builder or `finishDerive`
on a generator builder returns `Error.Provider`. The compiler still requires
exactly the requested trait, target and binder order, with no helper declarations. Imports needed
to render types from this snapshot may precede the impl; duplicate or unrelated
imports are rejected. Formatting composes mappings against the complete source,
including these imports, before publication.

`DeriveRequest.bounds()` returns the written request bounds as `"T: Trait"`
entries, in binder and authored bound order; the target's declaration bounds
remain available in the snapshot. Bound trait arguments retain their lexical
source spelling. These bounds and the target's required bounds must remain in
the generated header. Providers may add positive bounds on existing binders.
After checking the complete expansion, the compiler checks it again with each
added bound removed independently. An expansion still valid without that bound
is rejected with `E2105`. Explicitly written bounds are not subject to this
minimality test. Resource exhaustion or incomplete checking cannot prove a
bound necessary. Rechecks do not execute generated code, rerun providers or
create publication artifacts. The inspection query reports actual added
`"T: Trait"` bounds, and request provenance includes all written requirements.

`Field.typeRef`, tuple variant payload items, newtype and alias underlying types,
function and constant signatures, and `Operation.signature` are opaque `TypeRef`
values. Constants expose their declared type, never their initializer or value.
Functions expose signatures and generic bounds, never executable bodies.
Public inherent methods and associated functions use `DeclarationKind.Function`
with `Owner.method` as their declaration name. Declaration uniqueness includes
the type/value namespace derived from its kind; an ordinary type and value may
share a name. Ordering uses module, declaration name, then type before value.
Trait operations expose their method-local `genericParameters`, `hasDefault`
and `requiresSelfSend`. Type rendering retains the distinct positions of trait
parameters, `Self` and method-local parameters. Public generic bounds use
resolved trait identities; authored bounds remain internal inputs to standard
derive rendering and are included in the model hash.
Referenced prelude traits appear in module `prelude` with origins such as
`Builtin("prelude:Copy")`. Their operation sets, signatures and generic arities
come from HIR's protocol definitions. Marker and callable protocols have no
named operations; serialization protocols retain their method-local adapter
bounds. The closure follows generic bounds, visible implementation headers and
opaque-result bounds as well as ordinary field and function types.
`TypeRef.identity(): String`
returns the resolved identity, so transparent aliases compare identically;
nominal newtypes remain distinct. The request also owns the exact rendering
context. The standard derive adapter retains authored type spellings internally,
and those spellings are included in the snapshot hash alongside canonical
identity and rendering data.

`SourceBuilder.renderType(mut self, path: String, ty: TypeRef): String ! Error`
names a type in the declared output module and stages its imports. Same-module
references use local names. Other public types use deterministic module aliases
and exact declared package paths. Private types from another module, packages
without a direct dependency path and unavailable source names return
`Error.TypeRendering(String)`. A failed render leaves the builder unchanged.
Rendering after `add` rejects the already completed path. `add` prepends the
deduplicated imports in alias order and counts them in the UTF-8 source budget.
Imports are local to one output; their order does not depend on render-call order.

`Snapshot.environment` retains the consumer's explicit edition, target,
profile, capabilities, features and admitted package identities.
`Snapshot.implementations` contains visible implementation headers, generic
parameters, origins and documentation. It does not contain their bodies.

Declarations, fields, variants, operations and implementation headers expose
`origin: Origin`. `Origin.Source(Span)` identifies authored source;
`Origin.Builtin(String)` identifies a compiler-defined declaration or member
without inventing a source position. Referenced compiler-defined nominal types
retain their actual HIR fields and variants in the structural closure.
Diagnostic origins and source maps remain source spans. A builtin origin never
authorizes a source range; the actual source ranges of admitted declarations
and members, plus a derive request's own span, are eligible origins.

The compiler builds the pre-generation model from a projection of authored
signatures. It retains only imports and declarations needed by the explicit
roots, referenced types, derive requests and applicable implementation headers.
An actual signature dependency on a current-round output fails with `E2109`.
Unused imports and dependencies confined to function bodies or unrelated private
declarations do not create that dependency. Projection preserves original byte
positions for diagnostics. The final compilation checks the original complete
sources together with the accepted generated batch; the projection is never
executed or published.
Erased declarations can extend a preceding CST node's trailing trivia. The
compiler binds projected declarations back to their original token anchors and
restores authored derive and implementation spans before building requests.
Provider identity, source maps and request hashes use those authored ranges.

`stringLiteral(value: String): String` emits a quoted Tondo literal, escaping
quotes, backslashes, interpolation braces and Unicode controls.
`indentation(level: UInt32): String ! Error` emits four spaces per level and
rejects levels above 1,000,000 with `Error.Provider`. Both are ordinary Tondo
functions governed by the invocation's execution and memory budgets. Providers
can use them when constructing strings for `SourceBuilder.add`.

Successful ordinary compiler results expose `SemanticModel::meta_expansions`.
Its canonical `tondo-meta-query-0.1/1` document retains accepted final source,
provider and request hashes, introduced bounds and composed source maps.
Generated source IDs follow toolchain section 2.2: `derive:<request-hash>` for
an expansion and `gen:<request-hash>:<canonical-output-index>` for a generator.
Derive paths use the reserved `@generated/derive/<request-hash>.to` namespace.
The derive ID and reserved path are calculated from the invocation identity;
they are not recursively hashed into themselves. The input identity includes
the target, requested trait and bounds, authorizing span, lexical imports,
snapshot and locked provider budgets. Hashes cover the final formatted source,
including the imports needed to compile it in its owning module.

The public test route retains these accepted production expansions when it
seals the production graph. Test consumers reuse the sources and provenance;
they do not start another production generation round.

`GenerateResponse` contains `outputs: Map[String, String]` and
`diagnostics: Array[Diagnostic]`. `DeriveResponse` contains `source: String`,
`diagnostics: Array[Diagnostic]` and `mappings: Array[SourceMap]`. Derive source
must contain exactly the authorized complete `impl`, including its target,
trait and introduced generic bounds. Diagnostics have a closed severity,
message and optional authorized origin; error diagnostics publish no output.
Successful notes and warnings remain part of the canonical response identity.
For ordinary source providers, the VM output counter includes the complete
canonical JSON response before formatting. A generator response encodes
`outputs` (paths sorted by key) followed by `diagnostics`; a derive response
encodes `source`, `diagnostics`, then `mappings`. JSON escaping and UTF-8 bytes
count toward the budget. The source builder's earlier source-byte check is an
additional admission check, not the final response measurement.

The formatter can now return token provenance. The plan composes original
provider byte ranges through this mapping, preserving exact UTF-8 boundaries.
Removed trivia, rewritten token interiors, overlap and unrelated origins are
rejected; no replacement mapping to the whole file is invented. An explicitly
provided whole-source range retains its document boundaries.
The ordinary compiler carries these maps into primary and related diagnostics.
An unmapped or crossing range retains its generated location. A map never
authorizes a replacement edit to the original declaration. Provider origins
must also be valid UTF-8 boundaries in the admitted source database.

Source providers can now serve ordinary frontend `derive` requests through an
explicit compiler-owned registry keyed by exact package, module and trait name.
Each mapping retains its own locked limits and exact compiled artifact hash.
Notes and warnings survive final compilation; domain errors retain the request
as their primary location and authorized input locations as related entries.
Conventional TOML meta discovery and locked source loading are implemented in
the public `lock`, `run` and `test` routes. The public conformance checker requires
observations for the complete callable and requirement table in
[`stdlib-meta-reflect-conformance.md`](stdlib-meta-reflect-conformance.md).
Owner promotion remains pending until the current execution report and joint
gate pass; the table alone does not close the tracker leaves.
The manifest groups these owners under
`[meta.dependencies]`, `[[meta.inputs]]`, `[[meta.generators]]` and
`[[meta.derive_providers]]`.

## Evidence and budgets

The owner contract is [`testing/stdlib-meta.json`](../../testing/stdlib-meta.json)
and the current cell-by-cell record is
[`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json).
They keep `MODEL`, `TEST` and `FUZZ` separate from the implementation proof.
`HOST` is explicitly `not-applicable`: this package is admitted to the
build-only `tondo-meta` graph and has no runtime host adapter. Compile-time and
generated-source-size budgets are declared with the standard performance
regression budgets; capture remains a promotion gate rather than an invented
runtime benchmark.

The executable evidence is bounded by `meta_steps`, `meta_memory_bytes`,
`meta_output_bytes`, renderer indentation, and the protocol probe input limit.
Run `scripts/stdlib-meta-check.sh` and
`scripts/stdlib-owner-evidence-check.sh` before the normative matrix check.
