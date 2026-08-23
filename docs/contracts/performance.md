# Global performance baseline contract

`PERF-001` fixes the performance experiment before Tondo chooses a native
backend. It is a design contract, not a benchmark result and not a language
semantic guarantee. The canonical machine-readable record is
[`testing/performance.json`](../../testing/performance.json).

The standard-library owner contract remains the more detailed authority for
individual `std.*` hot paths in [`stdlib-performance.md`](./stdlib-performance.md).
This contract composes those owners with the compiler, VM, memory and hosted
program boundaries needed to compare the VM with the future native backend.

## Measurement identity

Every observation has one immutable logical identity:

~~~text
suite + workload_id + fixture_sha256 + target + backend + profile + toolchain + flags + git_revision
~~~

The fixture hash, bounds and declared dimensions are part of the workload
identity. A benchmark cannot silently change its source, input size, malformed
case, seed or limit and retain the old baseline. Changing a workload requires a
new contract revision and a new reviewed baseline.

The report records CPU model/features, memory, operating system/kernel, target,
backend, profile, compiler/toolchain, flags and source revision. Timestamp,
process ID, physical path, CPU frequency and ambient environment are never
identity fields. They may be diagnostic metadata when a runner needs them, but
they cannot make two otherwise equal measurements different or justify a
regression waiver.

## Protocol and bounds

The capture protocol is deliberately the same shape as `STD-PERF-001`:

- monotonic clock;
- three warmups;
- nine measurements in each of three independent processes;
- at least 27 samples per logical measurement;
- median, p95 and p99 summaries; and
- reported outliers, never silent deletion or re-running until the number looks
  acceptable.

The workspace is clean, the Rust toolchain is pinned, and the seed is
`tondo-perf-0.1`. Every workload has positive finite limits for source bytes,
steps, managed memory, output bytes and duration. The limits are safety bounds
for the harness; they do not change Tondo program semantics.

The async-selection leaf uses the same identity and sampling rules through
`testing/async-select-performance.json`. Its probe executes the verified VM
registration and commit path for ready and pending selections with 1, 2, 8
and 64 arms, and compares the one-arm path with a direct `Join` handoff. The
report preserves latency samples and records managed allocations, runtime
frame bytes, registrations, arm scans and wakeups. These counters are
diagnostic observations, not language semantics: they make linear frame
growth, the pending two-pass scan and one wakeup per parked selection
machine-checkable without exposing VM layout to Tondo programs.

The manifest includes four compile workloads and ten runtime workloads; two of
the fourteen are explicitly adversarial boundaries. The runtime set contains
representative STD-0.1A programs for core, collections, text, codecs, I/O,
process and bytes, plus async and memory-pressure cases, as well as the
runtime overflow boundary. The compile set covers the empty bootstrap, generic
calls, inferred suspension and malformed source diagnostics. Each fixture is
an existing `.to` file with a checked SHA-256, so the suite cannot drift merely
because a path still exists.

## Dimensions and budgets

The default maximum regression budgets are encoded in basis points in the
manifest:

| Dimension | Unit | Direction | Default maximum regression |
| --- | --- | --- | ---: |
| `compile_time` | milliseconds | lower | 10% |
| `code_size` | bytes | lower | 5% |
| `startup` | microseconds | lower | 10% |
| `throughput` | operations/s | higher | 5% |
| `latency` | microseconds | lower | 10% |
| `allocation_count` | count | lower | 0% |
| `allocated_bytes` | bytes | lower | 10% |
| `peak_memory` | bytes | lower | 10% |
| `retain_operations` | count | lower | 10% |
| `release_operations` | count | lower | 10% |
| `pause_time` | microseconds | lower | 10% |

The zero allocation-count budget is intentional: an owner must explicitly
review a newly allocating hot path instead of hiding it behind a faster
throughput result. A stricter owner budget is always valid. A looser budget
requires a new reviewed baseline and a documented trade-off; it is never
silently changed in a benchmark file.

Budgets are compared only between observations with the same workload hash,
target, backend, profile, toolchain and flags. Targets and backends are never
averaged into a single green number. Improving throughput cannot cancel a
memory, tail-latency, retain/release or pause regression.

## Scalar oracle and backend boundary

The VM-hosted backend is the first required baseline. The compiler oracle is
canonical interface/artifact bytes plus exact diagnostics. The runtime oracle
is exact Tondo language observables; allocation, retain/release and pause
counters are instrumented harness observations and never become language
semantics.

`native` is deliberately present as a deferred backend entry. `NATIVE-001` must
select and evaluate Cranelift, LLVM or a generation strategy using this exact
suite. It may compare performance only after every native workload matches the
VM oracle for values, errors, ordering, ownership, overflow, cancellation and
exit behavior. A faster but semantically different backend fails the gate.

SIMD, word-at-a-time kernels, lookup tables, specialization, automatic
vectorization and target multiversioning are permitted. They must retain the
portable scalar fallback and exact observations. Kernel selection is outside
the public API and cannot make a CPU feature, memory layout or unsafe binding a
required capability.

## Gate sequence

The contract has four gates:

1. **Design:** workload identities, fixture hashes, finite bounds, dimensions,
   budgets and oracle are reviewed before backend implementation.
2. **Capture:** a clean workspace and pinned toolchain produce the complete
   environment record and repeated samples.
3. **Compare:** exact oracle equivalence and every applicable budget pass for
   each target/backend; no sample exceeds a declared bound.
4. **Promote:** the baseline and report are reproducible, CI evidence exists,
   and there is no unexplained regression.

`PERF-001` closes only the first gate and the machine-checkable design needed to
run the remaining gates. It intentionally does not invent timing numbers or
claim that a native backend exists. The baseline capture is required before
`NATIVE-001`, `NATIVE-ABI-001` or promotion of an optimization.

## Failure policy

Missing or changed fixture bytes, duplicate workload IDs, unknown dimensions,
zero or overflowing bounds, forbidden identity fields, insufficient samples,
an unavailable environment field, oracle mismatch, a mixed target/backend or
an unexplained budget regression is a gate failure. The runner must report the
failure and preserve the old reviewed baseline; it must not delete samples,
change workloads, average incompatible machines or fall back to a slower
measurement that hides the defect.
