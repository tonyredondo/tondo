# ADR-018: TLF is a transport format, not a second language

**Status:** accepted

## Context

LLMs benefit from a compact representation, but accepting a second source
grammar inside the compiler would split parsing, diagnostics, tooling and
conformance. Token measurements also show that replacing familiar Tondo
keywords with shorter spellings rarely reduces subword-token counts.

## Decision

Tondo LLM Form is a public, deterministic transport format selected explicitly
outside `.to` source. It preserves Tondo token spellings, encodes logical
newlines compactly and expands to canonical Tondo before the ordinary lexer and
compiler pipeline.

The compiler has one language semantics. TLF cannot add inference, imports,
types, effects or runtime behavior, and normal commands never auto-detect it.

## Consequences

The codec can be optimized and versioned independently without creating Tondo
programs that only one backend understands. Formatter, diagnostics and source
maps remain shared. TLF cannot claim effectiveness from token counts alone; it
must pass round-trip, differential-compilation and model generation/repair
gates before being called stable.
