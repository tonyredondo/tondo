# ADR-015: The bootstrap subset is not a Tondo dialect

**Status:** accepted

## Context

Implementing Tondo incrementally must not create temporary syntax or semantics
that later programs depend on.

## Decision

The frontend recognizes edition 0.1. Unsupported features receive an explicit
implementation diagnostic under the `T` prefix. They are never reinterpreted.

## Consequences

Bootstrap releases cannot claim full conformance. Every completed feature moves
from explicit rejection to its normative behavior without source migration.
