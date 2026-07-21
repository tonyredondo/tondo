# ADR-005: Use explicit compiler IR stages

**Status:** accepted

## Context

Syntax, name resolution, inference, ownership, cleanup, and execution answer
different questions and need independent diagnostics.

## Decision

Use `CST -> resolved HIR -> typed HIR -> MIR -> verified bytecode`.

## Consequences

Every phase documents entry and exit invariants. MIR, not AST, becomes the input
to borrow analysis, cleanup lowering, async transformation, and backends.
