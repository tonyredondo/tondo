# ADR-013: Monomorphize initial generic code

**Status:** accepted

## Context

Tondo traits use static dispatch and do not expose dynamic trait objects.

## Decision

Instantiate generic functions and types for concrete substitutions when lowering
the initial implementation.

## Consequences

Bytecode stays simply typed and calls remain direct. The compiler enforces
termination and instantiation budgets to prevent unbounded expansion.
