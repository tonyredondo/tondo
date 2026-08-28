# Native AOT scope contract

`NATIVE-AOT-SCOPE-001` fixes the product scope for the first Tondo release
candidate. Its machine-readable authority is
[`testing/native-aot-scope.json`](../../testing/native-aot-scope.json), and
the static/negative gate is
[`scripts/native-aot-scope-check.sh`](../../scripts/native-aot-scope-check.sh)
with [`scripts/native-aot-scope-test.sh`](../../scripts/native-aot-scope-test.sh).

## Product boundary

The primary 0.1 product is a native AOT executable. `tondo-vm-hosted` remains
the reference implementation, differential oracle and bootstrap/hosted target;
it is not a second language semantic. JIT is outside the 0.1 product and is
not an input or scoring dimension for `DEC-013`.

Both candidate backends consume the same verified MIR, target descriptor,
runtime, stdlib, linker policy and workload identity. The contract includes
Cranelift and LLVM and excludes the custom generator until it has a comparable
machine-code adapter. `DEC-013` records Cranelift as the selected backend for
the admitted target; this scope contract does not self-claim N1 or turn a
bounded probe into a production benchmark. The independent N1 report consumes
the closed campaign and promotes only the primary x86_64 GNU target.

## Memory boundary

The language contract is collector-neutral. The native AOT runtime uses the
`hybrid-arc-cycle-collector` policy from `NATIVE-MEM-ADR-001`; the hosted VM
keeps the precise tracing, non-moving, stop-the-world mark-and-sweep collector
from `ADR-009`. Measurements must not compare these implementation details as
language behavior, and a VM measurement cannot substitute for native AOT
memory evidence.

## Comparable evidence

Every observation is keyed by edition, target, backend, profile, toolchain,
runtime, stdlib, fixture, flags and source revision. The AOT campaign keeps
compile latency, linked-binary size, startup, runtime, memory, diagnostics,
maintenance and distribution as separate dimensions. It never aggregates
targets or backends and never trades a regression in one dimension for an
improvement in another.

Binary size is measured from the same final linked product for both candidates:
stripped bytes, bytes with debug information and relevant section bytes are
reported separately. Raw Cranelift code-buffer lengths and complete LLVM object
file lengths are not comparable product-size measurements and cannot promote a
backend.

The full campaign uses the existing `PERF-001` protocol: three warmups, nine
samples in each of three fresh processes (at least 27 samples), monotonic
clocks, hash-bound fixtures and deterministic toolchain inputs. The next block
is `NATIVE-AOT-LOWER-001`; after lowering, `NATIVE-AOT-MEM-001` captures
process-local ARC/allocation/cycle/weak/pause/RSS observations from the linked
products, while the VM remains the semantic oracle. The required campaign
blocks are closed and feed the recorded `DEC-013` decision and the independent
Gate N1 promotion record.
