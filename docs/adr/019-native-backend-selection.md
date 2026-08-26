# ADR-019: Measure candidates before selecting the first native backend

- Status: Proposed — selection pending measured evidence
- Date: 2026-08-25
- Supersedes: none
- Next decisions: `NATIVE-002` (the physical thread lane is now closed)

## Context

Tondo has a verified CST → HIR → MIR → bytecode/VM pipeline and closed
contracts for native target descriptors, artifact graphs, link plans, publish
receipts and performance measurement. The next step needs a native codegen
backend, but selecting one is not the same as implementing lowering or an ABI.
The choice must preserve the VM's language observables and leave room for the
hosted diagnostic profiles (`race`, `leaks` and `crash`).

The candidates are:

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

No backend is selected yet. Cranelift and LLVM are measured candidates; the
custom generator is excluded from the ranking until it has a real machine-code
adapter. The fast lane in
[`tools/native-evaluation/`](../../tools/native-evaluation/) consumes the real
MIR probe and measures both engines over the same normalized module shape. A
sample is not semantic evidence when it contains trapped unsupported
functions; the report preserves those counts and keeps native equivalence
pending.

This is intentionally bounded:

- it keeps the optional Cranelift/LLVM evaluation dependencies outside the
  compiler workspace package graph;
- it does not promise a native executable, a stable object layout or public FFI;
- it does not create a second source language or a second semantic pipeline;
- it publishes exploratory compile-time and object-size samples only;
- it does not claim peak memory, runtime performance or semantic equivalence;
- it does not allow a native target into N1 until exact VM equivalence and
  diagnostic parity are demonstrated.

## Selection criteria

Cranelift is attractive because it is Rust-native, embeddable without a shell
or ambient tool discovery, and designed for fast code-generation iteration.
LLVM is attractive because of its mature target, debug and tooling ecosystem.
Neither advantage is a measured decision until both candidates consume the
same normalized MIR shape and their results are recorded under the same target
identity.

The custom generator is not a candidate for the fast ranking yet. Its smaller
dependency surface does not compensate for the correctness, unwind,
source-map, diagnostics and maintenance stack Tondo would have to own. It may
be reconsidered only with a real adapter and identical evidence.

The fast lane is a feedback mechanism, not a promotion gate. Final selection
requires full MIR lowering, VM/native semantic equivalence, full performance
capture and the normal quality gate.

The physical `Thread` lane is now an explicit prerequisite rather than an
assumption: the safe runtime launches a worker with a completion barrier and
the native differential runner proves the equivalent `pthread` lifecycle in
both candidates. This closes `NATIVE-THREAD-001` without selecting a backend.
The current adapter still evaluates an eager lowered value before handoff;
deferred callable-body lowering and scheduler coordination remain in
`NATIVE-002`.

## Required evidence before N1

The following are acceptance conditions, not optional future observations:

- all selected target descriptors and backend inputs are pinned and hashed;
- the complete conformance corpus and the `PERF-001` workloads compile and run;
- values, errors, ordering, ownership, overflow, cancellation and exit status
  match the VM oracle exactly;
- source maps, unwind, task/thread identity, memory/GC hooks and redaction are
  preserved;
- `DIAG-NATIVE-001` proves race/leak/crash behavior or marks the target
  explicitly limited and excludes it from N1;
- repeated compile/runtime/memory/size measurements use the existing identity,
  bounds and sample protocol;
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
not approximations. The native target/artifact/link/publish schemas remain
useful contracts but do not imply their implementation. A future ADR revision
can select Cranelift or LLVM per target if measured evidence justifies it.

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
