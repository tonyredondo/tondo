# ADR-007: Build a bytecode VM before a native backend

**Status:** accepted

## Context

The first implementation must validate language semantics quickly. Native code
generation, object formats, linkers, and platform ABIs add independent work.

## Decision

Compile the first executable vertical slice to Tondo bytecode and run it in an
interpreter.

## Consequences

The VM is a semantic oracle and test target. A native backend is selected only
after MIR and runtime behavior are stable.
