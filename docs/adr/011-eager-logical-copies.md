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

`tests/runtime/value-copy/` fixes that comparison at the public driver
boundary. It observes values, writes, identity, iteration, panic diagnostics,
exit status, output, and behavior under GC pressure without observing handles,
allocation counts, collection schedules, or storage strategy. The eager
runtime and every candidate COW runtime must satisfy the same unchanged
fixtures; an implementation that needs different expected output is not an
optimization.

Materialized array slices use that same rule. The bootstrap routes both a
direct slice operation and the materialization of a borrowed slice place
through one eager snapshot copier. It recursively copies ordinary `Copy`
elements, preserves the deliberate immutable `String` and `Ref[T]` identity
sharing rules, and allocates a distinct outer `Array`. A `ref` or `mut` slice
argument is not materialized by this path: it remains a temporary region loan
over the source array.
