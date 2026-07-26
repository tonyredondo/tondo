# ADR-009: Use precise tracing GC in the bootstrap VM

**Status:** accepted

## Context

Tondo requires automatic memory management that recovers unreachable cycles.
Implementing the planned native ARC plus cycle collector would delay execution.

## Decision

Use a precise, non-moving, stop-the-world mark-and-sweep collector in the
single-thread bootstrap VM.

## Consequences

The VM derives immutable object and frame descriptors from the closed verified
bytecode catalogs rather than trusting compiler-supplied tracing flags. Every
heap slot retains its descriptor identity; allocation, mutation, and marking
use the same checked shape. Active frames validate an exact typed slot schema,
and a future suspended frame can retain that schema unchanged.

The bootstrap representation remains deliberately explicit and more expensive
than a compact production layout. The native runtime may later use ARC plus
cycle collection without changing source semantics, provided it preserves the
same reachability and identity tests.
