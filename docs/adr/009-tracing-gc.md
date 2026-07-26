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

Current roots come only from active frame values and captured cleanup values,
scoped operation-local temporaries, pending object publication, and traced
managed edges. Every allocation-capable left-to-right evaluation retains its
completed values until publication or failure. Structured terminal traversal
uses the same scoped roots. The host boundary exchanges detached snapshots and
therefore owns no VM handles; suspended-frame containers are absent until M7
registers them explicitly.

A private test adapter uses the production heap, descriptor validation, roots,
allocation threshold, and mark-and-sweep pass. Under sustained allocation it
keeps a rooted cycle spanning a `Ref` cell, array, and closure environment,
reclaims equivalent unrooted cycles repeatedly, and reclaims the retained cycle
after root withdrawal. It does not expose a source intrinsic or an alternate
collector; public identity remains deferred to REF-001.

The bootstrap representation remains deliberately explicit and more expensive
than a compact production layout. The native runtime may later use ARC plus
cycle collection without changing source semantics, provided it preserves the
same reachability and identity tests.
