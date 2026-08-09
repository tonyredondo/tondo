# `std.bytes` contract

This document records the bootstrap implementation of the `std.bytes` slice
from Standard Library 0.1. It is intentionally smaller than the future codec
catalog: text encoding/decoding is strict UTF-8, while Base64, hexadecimal and
wire-format codecs remain owned by their later modules.

## Values

`Bytes` is an immutable host value. The source-visible constructors are `empty`,
`Bytes(String)`, and `fromArray`; every constructor and `toArray` copies the
input or output storage. `String(Bytes)` is the strict UTF-8 conversion back to
text. `BytesBuilder` is a mutable host value that is only accepted through
`var self` operations. A builder never exposes its storage and `finish` returns
a snapshot, so subsequent appends cannot change a finished `Bytes` value.

`BytesError` covers a rejected range, a byte-budget exhaustion, or a malformed
host boundary. `Utf8Error` is returned by strict `String(Bytes)`; no replacement
characters are inserted.

## Limits and algorithms

Each materialized byte buffer is bounded by the run's
`ResourceLimits.max_vm_heap_bytes`. Appends check the complete prospective size
before mutating and are atomic on failure. `Bytes.get` is total and returns
`none` for negative or out-of-range indices. `Bytes.slice` uses `[start, end)`
and returns `BytesError` for invalid bounds.

`Bytes.equal` compares bytes in order. `Bytes.hash` is the fixed FNV-1a 64-bit
algorithm. These properties are independent of host architecture and are
covered by the scalar reference tests and the executable runtime fixture.

## Host boundary

The compiler owns type identity and method signatures; the hosted VM owns the
opaque payload registry. Builder receivers arrive at the host as a verified
exclusive loan, are resolved to the same opaque token, and never become a
source-visible alias. Process output uses the same `String(Bytes)` conversion;
there is no process-specific text method.

## Evidence and budgets

The executable owner contract is [`testing/stdlib-bytes.json`](../../testing/stdlib-bytes.json);
its per-cell record is kept in [`testing/stdlib-owner-evidence.json`](../../testing/stdlib-owner-evidence.json)
under `STD-A-BYTES-EVIDENCE-001`. The evidence covers catalog shape,
ownership/snapshots, strict UTF-8, builder failure atomicity, limits/ranges and
properties/hot paths. `HOST` is explicitly `not-applicable`: `std.bytes` is an
intrinsic compiler/VM-owned value and has no separate provider capability.

The scalar implementation is the correctness oracle. SIMD or word-wide routes
may be promoted only after the same results, errors, limits and ownership
observables are demonstrated. Dedicated performance capture and fuzz promotion
remain pending; no throughput or allocation claim is inferred from unit-test
timing. Run the contract, negative, and owner-evidence checks with:

```text
scripts/stdlib-bytes-check.sh
scripts/stdlib-bytes-test.sh
scripts/stdlib-owner-evidence-check.sh
```
