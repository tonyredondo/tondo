# ADR-004: Combine recursive descent and Pratt parsing

**Status:** accepted

## Context

Declarations and type forms have clear recursive structure, while expressions
are most directly described by their normative precedence table.

## Decision

Use recursive descent for declarations, statements, patterns, and types, and a
Pratt parser for expressions.

## Consequences

Precedence is centralized. Contextual ambiguities produce preliminary CST nodes
instead of consulting inferred types during parsing.
