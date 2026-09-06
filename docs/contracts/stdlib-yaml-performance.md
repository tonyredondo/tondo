# `std.yaml` performance contract

`STD-YAML-PERF-001` closes the target-qualified performance boundary for the
Tondo 0.1 draft. The machine-readable contract is
[`testing/stdlib-yaml-performance.json`](../../testing/stdlib-yaml-performance.json).
Its status is `verified-hosted-vm-baseline`; the reproducible campaign completed
13 workloads with 27 samples per workload and the exact host/model checks below.
This promotion is limited to the hosted scalar baseline.

This is a reproducible `tondo-vm-hosted` / `bytecode-vm` baseline. It measures
the scalar YAML kernel through the real hosted bridge and does not promote the
public API, a native runtime, AOT lowering, SIMD, or a future multiversioned
route. The native AOT route remains outside this evidence boundary.

## Target and protocol

The private probe is
`process_host::tests::yaml_performance_probe` in
[`crates/tondo-compiler/src/process_host.rs`](../../crates/tondo-compiler/src/process_host.rs).
It runs three warmup iterations and nine measured repetitions in each of three
independent processes, for 27 monotonic-clock samples per workload. The fixed
batch is 16 operations. Outliers remain in `samples_ns` and are reported; they
are never removed to improve a percentile.

Report identity is limited to the declared suite, workload, probe hash, target,
backend, profile, toolchain, flags, and git revision. PID, path, timestamp,
ambient environment, and instantaneous CPU frequency are forbidden identity
inputs. Fixture setup is outside timed latency but remains in allocation and
logical-memory counters.

## Workloads and measurements

The 13 workloads intentionally include more than flat documents:

| Family | Workloads | Boundary exercised |
| --- | --- | --- |
| Materialized parse | Core scalars, nested maps/sequences, aliases, block scalars, multiple documents | YAML 1.2 Core resolution, frames, alias expansion, document boundaries |
| Borrowed parse | Nested `parseView` | borrowed lifetime and no published materialized value |
| Encoding | Nested normal encoding and canonical Core encoding | validation, deterministic rendering, UTF-8 key/scalar bytes |
| Streaming events | Reader and writer event cycles over block scalars | affine lifecycle, event conversion, buffered hosted bridge |
| Adversarial rejection | alias budget, depth budget, malformed flow collection | bounded rejection, error kind, no partial value |

Each sample reports input/output bytes, the fixed operation count, bridge bytes
copied, logical host-value allocations, peak logical memory, live affine YAML
handles, structural depth, aliases, expanded value nodes, the selected
`scalar-fixed-target` dispatch, and adversarial rejection count. Throughput is
derived from bytes copied and elapsed nanoseconds. Median, P95, and P99 tail
latency are retained for every workload. The contract calls this retained
distribution the tail latency evidence.

`logical_memory_bytes` is not RSS. It includes the host registry, YAML value and
byte payload capacities, and hosted stream output; allocator headers,
fragmentation, and unrelated process memory are excluded. Allocation counts are
logical host-value identities created by the fixture and operation, not OS
allocation counts. Reader and writer samples must finish with zero live affine
YAML handles.

The hosted Reader/Writer adapter is intentionally buffered in this draft:
`fromBytes` parses before exposing events and the hosted writer retains events
until `finish`. The event workloads therefore prove the existing hosted
contract and lifecycle, but do not claim a native incremental I/O runtime.

## Strategy boundary

The selected strategy is `scalar-kernel-host-bridge-baseline`. It is the only
executable YAML route available on this target. `native_runtime_abi` is
`not-measured-by-this-hosted-report`, `native_aot` is `not-claimed`, and SIMD
or optimized multiversion dispatch is `not-measured-no-optimized-route`.
The dispatch field records the declared target/workload size class; it is not
evidence that an optimized route exists.

The completed campaign executes the independent bounded YAML model and its
deterministic replay tests before collecting host samples. It checks exact
materialized values, canonical bytes, event counts, writer output, expected error
kinds, and terminal handle cleanup. It never combines targets or backends.

## Reproduction

```bash
scripts/stdlib-yaml-performance-check.sh
scripts/stdlib-yaml-performance-test.sh
TONDO_STDLIB_YAML_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-yaml-performance.sh
```

The runner writes
`target/reliability/evidence/stdlib-yaml-performance.json` and rejects a dirty
workspace by default. The promoted report records 13 workloads, 27 samples each,
the scalar-fixed-target dispatch, all declared latency/throughput/allocation and
adversarial counters, and zero live YAML handles at every terminal boundary.
YAML conformance is closed by `STD-YAML-CONF-001`; usage documentation, native
runtime behavior, SIMD and generic AOT remain separate frontiers.
