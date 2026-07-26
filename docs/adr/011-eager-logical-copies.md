# ADR-011: Implement eager logical copies before COW

**Status:** accepted

## Context

Value semantics are observable; copy-on-write is not. COW adds uniqueness,
aliasing, and mutation complexity.

## Decision

The VM may copy `Copy` composites eagerly until semantic tests are complete.

## Consequences

Correctness does not depend on reference counts. COW is introduced only after
benchmarks, with differential tests against eager copying.

The bootstrap implementation uses one exhaustive recursive copier for every
managed `Copy` shape. Immutable strings share their storage and `Ref[T]` shares
its identity cell; all other managed shapes allocate an independent object
graph while completed children remain rooted. These rules form the reference
implementation against which a later COW representation is compared.
