# ADR-008: Keep bootstrap values explicit

**Status:** accepted

## Context

NaN-boxing and compact tagged layouts improve density but complicate debugging,
GC, portability, and type invariants.

## Decision

Represent VM values with readable Rust enums and explicit heap references first.

## Consequences

The bootstrap spends more memory. Representation optimization requires profiles
and must pass the same observable runtime tests.
