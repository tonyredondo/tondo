# ADR-009: Use precise tracing GC in the bootstrap VM

**Status:** accepted

## Context

Tondo requires automatic memory management that recovers unreachable cycles.
Implementing the planned native ARC plus cycle collector would delay execution.

## Decision

Use a precise, non-moving, stop-the-world mark-and-sweep collector in the
single-thread bootstrap VM.

## Consequences

Frames and managed objects expose precise trace metadata. The native runtime may
later use ARC plus cycle collection without changing source semantics.
