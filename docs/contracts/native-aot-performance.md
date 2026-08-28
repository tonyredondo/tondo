# Native AOT performance contract

`NATIVE-AOT-PERF-001` is the repeated, target-qualified performance gate for
the complete linked AOT products. It is the final evidence block before the
human backend decision `DEC-013`; it does not select Cranelift or LLVM, alter
the language semantics, or promote Gate N1 by itself.

The machine-readable authority is
[`testing/native-aot-performance.json`](../../testing/native-aot-performance.json).
The static and negative checks are
[`scripts/native-aot-performance-check.sh`](../../scripts/native-aot-performance-check.sh)
and
[`scripts/native-aot-performance-test.sh`](../../scripts/native-aot-performance-test.sh).
The executable campaign is
[`scripts/native-aot-performance.sh`](../../scripts/native-aot-performance.sh).
It writes the path-free report to
`target/reliability/evidence/native-aot-performance.json` unless the caller
sets `TONDO_NATIVE_AOT_PERF_REPORT`.

## Scope and equivalence

The campaign consumes the same normalized MIR, runtime ABI, `STD-0.1A`, target,
linker and release profile used by `NATIVE-AOT-LOWER-001`,
`NATIVE-AOT-BINARY-001`, `NATIVE-AOT-MEM-001` and
`NATIVE-AOT-QUALITY-001`. Cranelift and LLVM each produce a complete linked
executable, not a code buffer or an object-only substitute. The VM and the
normalized-MIR reference interpreter remain the semantic oracles. A mismatch,
missing case, trap drift, non-reproducible product or unsupported admitted
operation fails closed before performance numbers are accepted.

The current adapter exposes two explicit workload identities. The
`aot-complete-product` workload is the admitted linked-product driver assembled
from the verified native product program and covers compile/lower, link,
end-to-end build completion, startup, throughput and latency. The
`aot-memory-workload` identity reuses the
instrumented linked-product observations already closed by
`NATIVE-AOT-MEM-001` for allocation, memory, ARC, cycle, weak-reference and
pause dimensions. This is an honest adapter boundary: it does not claim that
the native frontend has already compiled every one of the fourteen VM-level
`PERF-001` fixtures. Those fixtures remain the VM oracle corpus until the
frontend is admitted to the native product recipe.

JIT measurements, synthetic cross-target aggregation and automatic backend
selection are out of scope. The report always records
`selection: human-decision-required`.

## Measurement protocol

For each candidate and target, the adapter performs 27 isolated build
observations: three build cohorts (`process` 0..2), each with nine
repetitions. Every repetition has its own build directory and records compile
time, link time, end-to-end build time, object hash, debug size and stripped
size. The end-to-end timer starts immediately before backend code generation
and stops only after the stripped executable, its hashes and ELF metadata have
been written and validated. It therefore includes artifact finalization and
validation in addition to compile and link; it is measured for each paired
sample and its quantiles are never reconstructed by adding independent phase
quantiles. Debug and stripped products, ELF sections and object hashes must be
identical across repetitions; the first stripped product is used only as the
runtime executable.

Runtime measurements use that complete stripped product in fresh child
processes. Each of three process cohorts executes three warmups followed by
nine measured launches, yielding exactly 27 measured samples per candidate.
The driver must be silent and return successfully for every measured launch.
All timings use a monotonic clock. Summaries are median, p95 and p99; samples
are retained in full and are never discarded as outliers.

The normalized-MIR reference interpreter is measured separately with the same
3 × 9 protocol over the eight product cases currently supported by that
interpreter. Its 27 samples are stored under `vm_baseline` and are never mixed
into either native candidate's numbers. The remaining runtime cases are still
checked semantically by the quality oracle and are reported as explicitly
unsupported by this timing lane; they are not silently omitted. This is a
reference baseline, not a third backend candidate.

Memory and ARC dimensions are linked to the same candidate by the
`NATIVE-AOT-MEM-001` report. The memory lane has its own fresh-process 3 × 9
protocol and must contain exactly 27 validated observations before its
quantiles can enter this report. These counters are implementation
observations, not Tondo semantics.

## Dimensions and identity

The report publishes these dimensions for each candidate:

* `compile_time_ns`, `link_time_ns`, `build_end_to_end_ns`, `code_size_bytes`,
  `startup_ns`, `throughput_ops_per_second` and `latency_us` from the complete
  product; `build_end_to_end_ns` is the per-sample wall-clock interval from
  code generation start through final stripped-artifact validation;
* `allocation_count`, `allocated_bytes`, `peak_memory_bytes`,
  `retain_operations`, `release_operations` and `pause_time_ns` from the
  validated memory lane.

A comparable observation is identified by suite, workload ID, logical fixture
hash, target, backend, release profile, toolchain identity, flags and source
revision. Physical paths, process IDs, timestamps, ambient environment values
and payloads are forbidden in the persisted report. Results from different
targets or different workload hashes are never aggregated. A faster result in
one dimension cannot cancel a regression in another; allocation-count changes
always require explicit review. Unexplained regression or a missing bound
fails the gate.

The report deliberately separates the VM baseline from native measurements:
the VM establishes exact values, errors, ordering, ownership, cancellation and
trap behavior; the native lanes may only be compared after those observations
match. `PERF-001` supplies the global dimension budgets and workload frontier,
while this block supplies the complete linked-product evidence needed by
`DEC-013`.

## Reproducibility and promotion

The runner records logical toolchain and target identities, fixed seed
`tondo-native-aot-perf-0.1`, protocol counts and all retained samples. The
campaign is reproducible only when both candidates have 27 build samples, 27
runtime samples, 27 memory samples, equal product artifacts across builds,
positive finite dimensions (including the end-to-end build dimension) and
monotonic quantiles (`median ≤ p95 ≤ p99`).

`NATIVE-AOT-PERF-001` is closed only after the report, contract checker,
negative mutation suite and CI evidence pass. Closure means that the evidence
is ready for the human decision; it does not pick a backend, change the public
ABI, or claim support for an additional target.
