# ADR-001: Implement the compiler in Rust

**Status:** accepted

## Context

The compiler needs compact algebraic data structures, explicit unsafe
boundaries, predictable performance, and a mature systems ecosystem.

## Decision

The reference toolchain is implemented in Rust. M0 pins Rust 1.93.0 and declares
1.93 as its initial minimum supported version.

## Consequences

IRs use Rust enums and request-owned data. Unsafe Rust is forbidden across the
bootstrap workspace; a future runtime exception requires a new reviewed ADR.
