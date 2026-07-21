# ADR-012: Start with a non-incremental pipeline

**Status:** accepted

## Context

Incremental query systems require stable dependency boundaries that do not yet
exist and can hide nondeterministic invalidation defects.

## Decision

Compile each request through a deterministic clean pipeline during bootstrap.

## Consequences

Cold builds are slower. Phase inputs and outputs remain suitable for later
caching, and cache hits must be observationally identical to clean builds.
