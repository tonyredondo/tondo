# ADR-014: Keep bootstrap bytecode in memory

**Status:** accepted

## Context

A serialized artifact would prematurely freeze encoding, metadata, compatibility,
and loader behavior.

## Decision

Pass typed bytecode structures directly from compiler to VM during bootstrap.

## Consequences

There is no standalone bytecode file or compatibility promise. A disassembler
may exist for tests without defining an ABI.
