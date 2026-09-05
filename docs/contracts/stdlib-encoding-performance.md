# `std.encoding` performance contract

`STD-ENCODING-PERF-001` closes the target-qualified performance boundary for
the Tondo 0.1 draft. The machine-readable contract is
[`testing/stdlib-encoding-performance.json`](../../testing/stdlib-encoding-performance.json).
This is a reproducible hosted VM baseline; it does not promote the public API
or select a native implementation.

## Target and protocol

The private probe is
`process_host::tests::encoding_performance_probe` in
[`crates/tondo-compiler/src/process_host.rs`](../../crates/tondo-compiler/src/process_host.rs).
It invokes the real `std.encoding` host bridge and the scalar stdlib kernel on
the declared `tondo-vm-hosted` / `bytecode-vm` target. Three warmup iterations
and nine measured repetitions run in each of three independent processes, so
every workload has 27 monotonic-clock samples. Fixture setup is outside timed
latency but remains in allocation and logical memory counters. The deterministic
seed, batch size, sample count and identity fields are fixed in the JSON
contract. Outliers remain in the sample list and are reported rather than
deleted to improve a percentile.

Report identity includes suite, workload, probe hash, target, backend, profile,
toolchain, flags and git revision. PID, path, timestamp, ambient environment and
instantaneous CPU frequency are forbidden identity inputs.

## Workloads and measurements

The 16 workloads cover:

- materialized Base64 standard, URL-safe and unpadded decode paths;
- materialized hexadecimal lower and upper decode paths, including any-case
  input;
- one-byte and quantum-sized streaming boundaries; and
- empty, quantum, small and large payload classes.

Each sample reports input/output bytes, chunk size, monotonic latency, the
fixed 64-operation batch, logical bridge bytes copied, host-value allocations,
logical registry/payload memory, live encoding handles and the selected
`scalar-fixed-target` dispatch. Throughput is derived from bytes copied and
elapsed nanoseconds. Logical memory is not RSS: it includes host registry
entries and `Bytes` capacity, but excludes allocator headers and fragmentation.
The report retains median, P95 and P99 tail latency for every workload.
The probe compares every result with a deterministic scalar fixture, consumes
all returned `Bytes`, terminates every affine stream and requires zero live
encoding handles before returning.

## Strategy boundary

The selected strategy is
`scalar-kernel-host-bridge-baseline`. It is the only executable encoding route
currently available. `native_runtime_abi` is
`not-measured-by-this-hosted-report`; `native_aot` is `not-claimed`; and SIMD
and optimized multiversion dispatch are
`not-measured-no-optimized-route`. The report therefore does not claim code-size
or SIMD-crossover measurements; native AOT remains outside this report. The
dispatch object records target-declared
size classes and selects only the scalar route; it is not evidence of a future
optimized implementation.

The independent oracle is the bounded reference model and its hosted
regressions in
[`crates/tondo-reliability/src/encoding_model.rs`](../../crates/tondo-reliability/src/encoding_model.rs)
and
[`crates/tondo-reliability/tests/encoding_models.rs`](../../crates/tondo-reliability/tests/encoding_models.rs).
The runner executes that oracle before collecting host samples. It never
combines targets or backends.

## Reproduction

```bash
scripts/stdlib-encoding-performance-check.sh
scripts/stdlib-encoding-performance-test.sh
TONDO_STDLIB_ENCODING_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-encoding-performance.sh
```

The runner writes
`target/reliability/evidence/stdlib-encoding-performance.json` and rejects a
dirty workspace by default. CI runs the same runner from a clean checkout.
The next conformance leaf must provide VM/native interoperability, streaming
equivalence and scalar/SIMD evidence; this baseline alone does not close that
frontier.
