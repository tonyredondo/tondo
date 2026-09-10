# Sealed unit-overlay contract

**Status:** production semantic reuse is verified through the public hosted
test pipeline and its source-bound draft observations. `UTEST-OVERLAY-001`
and T0 are closed at the scope recorded in the implementation tracker.

`tondo_compiler::test_overlay` models the policy boundary for a unit companion
(`src/math_test.to`) over its production module (`src/math.to`). The production
phase is represented by immutable `ProductionSeal` metadata. This policy validator
only consumes that seal and companion metadata; it has no source database,
package graph or resolver input and therefore cannot reopen production bodies.

## Metadata seal

The model accepts a seal only when its caller supplies all of these facts:

- resolution is complete;
- semantic checking is complete;
- coherence checking is complete; and
- valid SHA-256 identities for the public interface, derived capabilities,
  coherence and production artifact.

The model's seal records the exact production source files and resolved
declarations in the module, including private declarations. `ProductionSeal::from_resolved`
is the adapter for the existing resolver output and requires the production
file set explicitly, so a companion that happens to share the module path
cannot enter the seal accidentally.

## Executable production reuse

The CLI accepts a production snapshot only from a successful ordinary compiler
output with complete expression checking and a compiled interface. The driver
binds its edition, target, profile, capabilities, features, package identities
and dependency aliases to the consumer graph. Source bytes, origins and logical
identities must agree with the captured production inputs.

Production sources, including compiler-generated standard and meta sources,
remain an unchanged `FileId` prefix. Companion sources are appended and their
diagnostic origins are remapped. `resolve_extension` preserves production
symbol, member, local and reference IDs. `lower_types_extension` preserves the
checked type arena, constants, inferred callable signatures, bodies and control
IDs. Production syntax is absent from both extension inputs and from expression
checking. The completed combined HIR still passes the ordinary IR verifier
before MIR and bytecode lowering.

The driver rejects unit coherence implementations and additions to sealed
production types with `E2003`. Empty extensions reuse the seal; generated JSON
sources retain access to the rest of the selected standard library. A test
consumer cannot discard a production file from the sealed prefix.

Regression tests execute a private production helper while the new syntax
budget is too small to parse production again. They also cover retained HIR
identity, alias and constant reuse, closures and loop/scope IDs, invalid
production, changed sources/package identity, new type and ownership errors,
coherence changes, generated standard sources, and empty extensions.

The pure metadata model below remains useful for policy tests. Its supplied
proof booleans and hashes do not create an executable compiler snapshot. The
public driver obtains that snapshot from actual compilation.

## Overlay rules

`build` accepts only `TestSourceClass::UnitTest` with the same package and
module as the seal. It then validates and deterministically orders:

- private helper declarations (`PrivateConstant`, `PrivateFunction`,
  `PrivateType`, `PrivateAlias`, `PrivateEnum` and `PrivateTrait`);
- explicit imports whose supplied interface surface contains only public
  declarations;
- references to private or public production declarations, private helpers,
  and imported public declarations; and
- the separate static suite/test tree, when present.

An overlay cannot publish a declaration, collide with a production declaration
or another helper, import its companion, access a private imported declaration,
or add a `CoherenceImplementation`. A production source class is rejected at
the boundary. Test/suite nodes remain in their separate registration namespace
and never collide with production symbols.

The resulting `UnitOverlay` carries the original seal by value. Its production
hashes, source set and declaration metadata are therefore exactly the sealed
values; the overlay does not calculate a new production artifact or alter
public interface, capabilities or coherence.

## Determinism and negative guarantees

Imports are ordered by alias, helpers by namespace/name/span and references by
source span and name. All validation is host-free and independent of input
vector order. An incomplete or invalid production proof fails before overlay
validation, so a test source cannot repair invalid production. The compiler
tests cover private companion access, explicit public imports, package/module
mismatches, public exports, collisions, self-imports, coherence mutation,
unknown/private references, source-class rejection and the resolver adapter's
production-file filter.
