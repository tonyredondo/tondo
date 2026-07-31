# ADR-004: Combine recursive descent and Pratt parsing

**Status:** accepted and portable

## Context

Declarations and type forms have clear recursive structure, while expressions
are most directly described by their normative precedence table.

## Decision

Use recursive descent for declarations, statements, patterns, and types, and a
Pratt parser for expressions. The readable recursive path is bounded to a
fixed shallow implementation depth. All source-controlled continuation beyond
that point is represented by explicit heap-backed frames, including Pratt
operators, delimiter forms, nested blocks/loops, calls, records, types, and
patterns. The logical `ParseLimits.max_nesting_depth` budget is charged by those
frames; it is not tied to the host thread stack.

## Consequences

Precedence is centralized. Contextual ambiguities produce preliminary CST nodes
instead of consulting inferred types during parsing. The explicit spill path
preserves the same CST shape, diagnostics, recovery, and formatter input as the
shallow path, while giving the parser O(depth) heap state and portable behavior
on small worker stacks.
