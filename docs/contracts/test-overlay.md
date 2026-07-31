# Sealed unit-overlay contract

**Status:** implemented for `UTEST-OVERLAY-001`

`tondo_compiler::test_overlay` is the semantic boundary for a unit companion
(`src/math_test.to`) over its production module (`src/math.to`). The production
phase is represented by an immutable [`ProductionSeal`]. The overlay validator
only consumes that seal and companion metadata; it has no source database,
package graph or resolver input and therefore cannot reopen production bodies.

## Production seal

A seal is accepted only when the production phase supplies all of these facts:

- resolution is complete;
- semantic checking is complete;
- coherence checking is complete; and
- valid SHA-256 identities for the public interface, derived capabilities,
  coherence and production artifact.

The seal records the exact production source files and the resolved declarations
in the module, including private declarations. `ProductionSeal::from_resolved`
is the adapter for the existing resolver output and requires the production
file set explicitly, so a companion that happens to share the module path
cannot enter the seal accidentally.

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
