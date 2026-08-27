# Native AOT linked-product contract

`NATIVE-AOT-BINARY-001` closes the size and reproducibility boundary between
the common MIR lowering and the later memory, quality and performance gates.
It measures products that a user could actually execute.  An object file, a
Cranelift code buffer or an LLVM bitcode/object size is not a product-size
observation and cannot be used to rank candidates.

The machine-readable authority is
[`testing/native-aot-binary.json`](../../testing/native-aot-binary.json).
The static and mutation checks are
[`scripts/native-aot-binary-check.sh`](../../scripts/native-aot-binary-check.sh)
and [`scripts/native-aot-binary-test.sh`](../../scripts/native-aot-binary-test.sh).
The executable evidence is emitted in the `native_aot_binary` field of
`target/reliability/evidence/native-evaluation-runner.json` by
[`scripts/native-evaluation-runner.sh`](../../scripts/native-evaluation-runner.sh).

## One product recipe

Both candidates receive the same `tondo-mir-backend/1` program, target
descriptor, release profile, runtime harness, standard-library identity and
link flags.  Only the machine-code adapter differs:

1. Cranelift emits one object containing the complete admitted AOT corpus.
2. LLVM `llc` emits one object from the same normalized MIR-derived module.
3. The same explicit C driver links each object with the runtime and product
   entry point, without a shell or `PATH` lookup.
4. The product contains 29 functions in its closed inventory: 28 admitted
   executable cases (7 storage/ABI cases and 21 runtime cases) plus one
   explicit trap for an unsupported opaque-storage function.  The entry point
   resets the private runtime between cases and executes every admitted
   storage, collection, projection, closure, call, cleanup, ownership, async,
   select and thread case.  A failed case or a missing symbol makes the
   product invalid; a partial product is never counted.
5. The linked debug product is copied and passed through the explicitly chosen
   `strip --strip-debug` tool.  The two byte streams are measured separately.

The runtime and standard-library values in this block are logical identities,
not a promise that the temporary C harness is the production ABI.  The runtime
identity is `tondo-runtime-draft/1` and the hosted foundation identity is
`STD-0.1A`; their hashes are part of every receipt.  The target descriptor is
the closed Linux x86-64 release target until another target has its own
registry entry and physical smoke.

## Measurements

For each candidate the report contains two fresh builds.  Each build records:

- object, linked-debug and linked-stripped SHA-256 identities;
- byte counts for the debug and stripped products;
- section sizes from the explicit `readelf` executable, including `.text` and
  the debug sections;
- compile and link elapsed time for observability only; and
- a receipt hash binding all inputs, flags and output identities.

The stripped product is launched in three fresh processes.  The report stores
the three monotonic startup samples plus median, p95 and p99.  This is a small
product-validity sample, not the final `PERF-001` campaign: the latter owns the
three warmups, nine samples and three-process repeated workload protocol.

The two builds must have identical object, debug, stripped and section hashes,
and identical receipt hashes.  Prefix-map flags remove the build directory
from debug records; any reproducibility mismatch is a hard failure.  All
physical paths stay in orchestration arguments and are absent from receipts
and reports.

## Receipt and identity

`tondo-native-aot-binary-receipt/1` is canonical compact JSON in struct order.
It binds:

- candidate and target/profile;
- normalized-MIR, runtime, standard-library and target-descriptor hashes;
- linker, strip and section-reader hashes and the exact logical link flags;
- candidate toolchain hash;
- object hash; and
- debug/stripped hashes and byte counts.

The receipt hash is over those fields and contains no physical path, timestamp,
process ID, address or ambient environment value.  A report with a missing
candidate, zero-byte product, missing `.text`, invalid hash, mismatched receipt,
or an unexpected output stream is rejected before it can feed backend
selection.

## Scope and non-goals

This block proves that both candidates can produce the same complete linked
product recipe and that its identity is reproducible.  It does not select a
backend, claim N1, measure native memory/ARC, run the complete quality corpus,
  or publish a performance winner.  Memory and quality are now closed by
  `NATIVE-AOT-MEM-001` and `NATIVE-AOT-QUALITY-001`; repeated performance
  remains `NATIVE-AOT-PERF-001`.
