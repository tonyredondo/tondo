# Semantic query contract

**Status:** complete Tondo 0.1 query surface implemented and conformance-pinned

## Snapshot boundary

`CompilationOutput::semantic_model` exposes an immutable `SemanticModel` tied
to the exact source snapshot that produced the compilation report. The model
owns its `SourceDatabase`, `ResolvedProgram`, and, once type lowering has run,
its `HirProgram`; it never borrows the consumed `CompilationRequest`.

The availability boundary is explicit:

- lexical and syntax rejection, formatting, and pre-frontend resource
  rejection do not expose a semantic model;
- a name-resolution rejection exposes sources and its partial resolved
  program, but no typed HIR;
- type-lowering and expression-checking rejection exposes the semantic state
  completed up to that point; and
- a semantically accepted bootstrap module exposes the complete typed HIR and
  lets module-mode `Operation::Check` complete successfully; script/fragment
  checks, `run`, and explicitly deferred semantic surfaces still emit `T0001`.

`expression_check_complete` is independent of diagnostics. It is true only
when the bootstrap checker understood every expression surface in the
snapshot. A false value never authorizes tooling to guess the missing fact.

## Identity model

`FileId`, `SymbolId`, `MemberId`, `LocalId`, `HirExpressionId`, and `TypeId` are
handles scoped to one `SemanticModel`. They are valid for direct lookup inside
that model and are never stable serialized identities. Global declarations
retain their complete `SymbolIdentity`; types retain their canonical recursive
serialization.

`SemanticEntity` distinguishes:

- resolved names, including globals, locals, receivers, prelude names, and
  source-less external names;
- resolved members;
- modules; and
- unresolved contextual type/value candidate pairs in a partial snapshot.

Separate language namespaces are not collapsed. A newtype declaration can
therefore return both its type symbol and its value constructor at the same
source range. A shorthand record pattern can likewise return both its field
member and newly declared local.

## Source selection

All ranges are byte-based and semi-open. `entities_at` queries an exact name
token range. `entities_containing` selects the narrowest name occurrence under
an offset and retains every distinct entity tied at that range.

Typed CST nodes can include leading trivia. Expression and call range queries
therefore first use an exact HIR span and otherwise select the smallest typed
node that covers the supplied visible range. Offset queries use strict
half-open containment. When contextual coercion and its operand share a span,
the later outer HIR node wins, so the reported expression type is the type
visible to its consumer rather than the pre-coercion implementation detail.

## Implemented queries

The request-owned snapshot provides direct structural queries for:

- source type annotations;
- expression IDs, expressions, and their contextual static types;
- entities at declaration and use sites;
- declaration spans;
- use-site references of globals, locals, modules, fields, enum variants, and
  other checked members;
- callable signatures for global functions, inherent methods, and any later
  callable already represented by `HirCallableId`;
- the direct declaration signature of a checked call when its callee retains a
  callable identity;
- normalized union members;
- enum variants and payload shapes; and
- the closed error set of a checked call;
- the `Absent`/`Potential`/`Present` terminal status of an interned type; and
- the sealed consuming-operation/unwind contract when that type is itself a
  direct language-owned terminal root.

The portable `semantic-snapshot` view additionally publishes every §22.5 fact
for one logical file:

- the six closed capability results and structural terminal origin for every
  relevant type;
- closure signatures, `Call`/`CallMut`/`CallOnce`, captures, and environment
  capabilities without captured values;
- opaque-result identities and bounds without their private witnesses;
- public record constructibility and the opaque effect of private state on
  equality and hashing;
- iterator protocol, concrete cursor, and unique element type;
- ownership mode and forcing patterns for `match`;
- `ref`, `mut`, and `var` parameter/pattern origin, lexical region, and
  structured end;
- MIR loans with origin place, mode, reservation sites, and call or lexical
  release boundary;
- retained and elided overlap/duplicate checks, with the static proof attached
  to every elided check;
- source-visible affine values and their CFG-local available, reserved, moved,
  and consumed events;
- each `Join`, its owning task scope, transfers, and observed consumption
  state;
- explicit, callable, and closure `unsafe` regions plus the operation requiring
  each region;
- option/result spelling and dot-call expansion facts; and
- canonical formatter bytes and their SHA-256.

References exclude the declaration itself. They are sorted by stable logical
`source_id`, module, path, start byte, and end byte, independently of source
insertion order. Member occurrences are recorded by the typechecker at the
exact token where field or variant selection becomes unambiguous; tooling does
not repeat that resolution over the CST.

Enum payload types are declaration templates. `SemanticTypeMembers::Enum`
returns the concrete arguments of the queried nominal instance alongside those
templates, so generic positions remain structural and unambiguous without
minting query-time `TypeId` values. Union members are already flattened,
deduplicated, stripped of `Never`, and canonically ordered by the type interner.

A closed call error query returns:

- `None` for a non-call, missing HIR, or a recovery/inference outcome;
- an empty vector for an infallible call or a `Result` whose error type is
  `Never`;
- one type for a single error; or
- the canonical member order for a union error.

## Portable serialization

The conformance adapter serializes this surface as
`tondo-semantic-observation-0.1/1`. A file-wide result uses
`tondo-semantic-snapshot-0.1/1`; its MIR-owned subsection uses
`tondo-semantic-ownership-0.1/1`.

Every serialized semantic identity begins with `sem:` and is derived from
logical source identity, canonical declaration identity, canonical type, and
the smallest necessary structural discriminator. Request-local arena, block,
local, loan, and scope indices are never emitted. Source spans contain exactly
`source_id`, module, logical file, start byte, and end byte. Repeated
collections have a normative, validated order.

The structured view exposes only semantic types and capabilities. Private field
names appear only when the query originates inside their owning source
snapshot; public type summaries expose constructibility and opaque
equality/hash participation without revealing private values or physical
layout.

AST formatting remains the formatter's lossless-CST operation. A format request
does not run semantic analysis merely to populate this model.

## Validation

Unit and public integration tests cover contextual coercion selection, visible
ranges that exclude CST trivia, global and local declarations and references,
method receiver signatures, namespace ambiguity, field and enum-pattern member
occurrences, shorthand field/local overlap, generic enum payload templates,
normalized union members, infallible/single/union/`Never` call error sets,
terminal status and direct `Join` contract, partial snapshot availability,
half-open boundaries, and logical reference ordering across files inserted in a
different order. The Tondo 0.1 conformance group additionally pins the complete
JSON schema, stable IDs, spans, ordering, closure/opaque/public-type facts,
borrow and affine lifecycles, required and elided dynamic checks, structured
`Join`, `unsafe`, sugar, and formatter output. Its safe-fix case replays the
published byte edits against the exact source snapshot and requires a second
error-free check.
