# ADR-006: Use slot-based bytecode

**Status:** accepted

## Context

An operand stack is small but obscures value identity, root locations, moves,
and source-level temporaries.

## Decision

Bytecode instructions read and write explicit typed frame slots.

## Consequences

Bytecode is larger but easier to verify, disassemble, trace, and map back to MIR.
The VM can enumerate roots without reconstructing stack types at each offset.
