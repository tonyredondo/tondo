# ADR-019: Select Cranelift as the first native backend

- Status: Accepted — Cranelift promoted by Gate N1 for Tondo 0.1 AOT on
  `x86_64-unknown-linux-gnu`; ARM64 remains a candidate smoke target
- Date: 2026-08-28
- Supersedes: none
- Decision record: `DEC-013`
- Next decision: STD-0.1B Group conformance (`STD-ASYNC-GROUP-CONF-001`); the
  hosted Group implementation, model/tests/fuzz and target-qualified
  performance budget are closed, while a future target needs its own complete
  AOT async evidence before promotion

## Context

Tondo has a verified CST → HIR → MIR → bytecode/VM pipeline and closed
contracts for native target descriptors, artifact graphs, link plans, publish
receipts and performance measurement. The next step needs a native codegen
backend, but selecting one is not the same as implementing lowering or an ABI.
The choice must preserve the VM's language observables and leave room for the
hosted diagnostic profiles (`race`, `leaks` and `crash`).

The 0.1 product scope is native AOT. `tondo-vm-hosted` remains the reference
implementation, bootstrap/hosted target and differential oracle; it is not a
second semantic pipeline. JIT is explicitly outside 0.1 and is not a backend
candidate or scoring dimension for `DEC-013`. The language remains
collector-neutral: native AOT follows `hybrid-arc-cycle-collector`, while the
hosted VM keeps its precise tracing collector.

The candidates evaluated for the admitted target were:

1. **Cranelift**, embedded as a pinned Rust dependency and driven directly by
   the MIR lowering pipeline.
2. **LLVM**, either through a pinned Rust API or a pinned external toolchain.
3. **A custom generator**, owned entirely by the Tondo compiler.

The evidence boundary is the real MIR, not a hand-written toy IR. The probe
[`native_mir_probe.rs`](../../crates/tondo-compiler/examples/native_mir_probe.rs)
compiles four hash-pinned fixtures through `Operation::Run`, and records a
stable summary plus the VM observables without leaking machine-specific
identity. The probe also carries a `tondo-mir-backend/1` normalized boundary.
The adapter lowers scalar comparisons and checked arithmetic, logical
operators, numeric conversions, verified direct calls, opaque host/aggregate
carriers, normal control flow (including loop-carried locals), and the
structured async/cleanup/select edges. The core fixture includes a pure scalar
operator/branch/loop matrix so each executable lowering is exercised by the
opt-in runner without changing program output; collection `IteratorNext`
remains an explicit fail-closed capability until the native stdlib storage ABI
is selected.

## Decision boundary

`DEC-013` selects **Cranelift** as the native AOT backend for
`x86_64-unknown-linux-gnu` in Tondo 0.1. LLVM remains an experimental
comparison backend and the custom generator remains excluded from the ranking
until it has a real machine-code adapter. The fast lane in
[`tools/native-evaluation/`](../../tools/native-evaluation/) consumes the real
MIR probe and measured both engines over the same normalized module shape. The
AOT campaign closed the required evidence boundary: samples with trapped
unsupported functions remain non-semantic evidence, while the complete linked
product, memory, quality and performance reports are bound to the decision.

Selection was not promotion until the compositional Gate N1 record passed.
`scripts/native-n1.sh` now promotes Cranelift only for
`x86_64-unknown-linux-gnu`; no compiler path may silently fall back to LLVM or
another backend. The selection remains target-scoped; a future target requires
its own evidence and decision record.

This is intentionally bounded:

- it keeps the optional Cranelift/LLVM evaluation dependencies outside the
  compiler workspace package graph;
- it does not promise a stable object layout or public FFI;
- it does not create a second source language or a second semantic pipeline;
- it publishes exploratory compile-time/object-size samples and a bounded
  executable semantic slice;
- it evaluates only the native AOT product path; JIT is not introduced as a
  hidden third candidate;
- it does not claim a public release, stable object layout or public ABI;
- it does not allow an additional native target into N1 until exact VM
  equivalence and diagnostic parity are demonstrated for that target.

## Selection criteria

Cranelift is selected because it is Rust-native, embeds without a shell or
ambient tool discovery, and keeps the first backend's implementation and
distribution surface small. In the completed AOT campaign its runtime
dimensions stayed within one percent of LLVM and its stripped product was
slightly smaller. LLVM's build-time advantage is useful for comparison, but it
does not offset the additional toolchain/FFI burden for the first backend.
These observations are scoped to the current target and workload; they are not
a claim that Cranelift wins every future target or workload.

The scoring scope is AOT-only. Linked executable size, startup, runtime and
memory observations must come from the same final product shape; raw code
buffers and intermediate object files are diagnostic observations, not a
selection metric.

The custom generator is not a candidate for the fast ranking yet. Its smaller
dependency surface does not compensate for the correctness, unwind,
source-map, diagnostics and maintenance stack Tondo would have to own. It may
be reconsidered only with a real adapter and identical evidence.

The fast lane and bounded runner remain evidence mechanisms, not an automatic
promotion gate. The human `DEC-013` record and the compositional N1 report are
now present; repeated performance capture and the normal quality gate are
retained as the evidence that justified this promotion.

The physical `Thread` lane is now an explicit prerequisite rather than an
assumption: the safe runtime launches a worker with a completion barrier and
the native differential runner proves the equivalent `pthread` lifecycle in
both candidates. This closes `NATIVE-THREAD-001` and contributes evidence to
the Cranelift decision.
The physical thread adapter still evaluates an eager lowered value before
handoff. `NATIVE-002` now closes the minimum deferred direct-task body path
(`spawn call()` publishes `Pending`, then `Join` completes and awaits it), while
mutable captures, closures, full scheduler coordination and native storage
remain outside this decision.

## Required evidence before N1

The following are acceptance conditions, not optional future observations:

- all selected target descriptors and backend inputs are pinned and hashed;
- the complete conformance corpus and the `PERF-001` workloads compile and run;
- values, errors, ordering, ownership, overflow, cancellation and exit status
  match the VM oracle exactly;
- source maps, unwind, task/thread identity, memory/GC hooks and redaction are
  preserved;
- `DIAG-NATIVE-001` proves race/leak/crash behavior or marks the target
  explicitly limited and excludes it from N1; its eight-case Cranelift/LLVM
  envelope parity is now closed for the current target;
- repeated compile/runtime/memory/size measurements use the existing identity,
  bounds and sample protocol;
- linked binaries report stripped, debug and section bytes from the same final
  product rather than mixing intermediate representations;
- native artifacts and publication remain deterministic and path-free.

## Consequences

The adapter now replaces the old count-driven smoke body with real normalized
MIR intake and common lowering in both measured candidates. The opt-in runner
compares native scalar values and traps with both the normalized interpreter and
a direct bytecode-VM invocation, and separately exercises managed results,
cleanup/ownership, structured async and selection. Checked-overflow and
logical-operator behavior, conversions, explicit-panic traps, loop-carried
locals and branch joins are covered for the executable slice. Collection
iteration and concrete aggregate storage remain explicit fail-closed leaves,
not approximations. The native target/artifact/link/publish schemas feed the
closed N1 record but do not themselves promote a target. Production native
lowering for the admitted target now proceeds through Cranelift; the LLVM
adapter remains available for differential testing and experimental comparison.
A future target or a materially different workload requires new evidence and
may receive a separate ADR without changing this target's decision.

## Evidence

- `testing/native-evaluation.json` — machine-readable decision and matrix.
- `testing/native-evaluation-fast.json` — short feedback-loop protocol.
- `scripts/native-evaluation-check.sh` and
  `scripts/native-evaluation-test.sh` — static contract and negative cases.
- `scripts/native-evaluation-fast.sh` and
  `scripts/native-evaluation-fast-test.sh` — measured candidate adapter and
  fast-lane negatives.
- `target/reliability/evidence/native-evaluation-fast.json` — current
  exploratory Cranelift/LLVM samples; it is not a selection or promotion
  record.
- `target/reliability/evidence/native-evaluation.json` — generated opt-in
  report, containing the real-MIR probe and toolchain observations.
- `testing/native-selection.json` and
  `scripts/native-selection-capture.sh` — bind the fast and executable
  reports to the Cranelift selection while keeping `n1_claim` false.
- `testing/native-aot-scope.json` and
  `scripts/native-aot-scope-{check,test}.sh` — close the AOT product boundary,
  memory distinction and comparable measurement protocol without selecting a
  backend.
