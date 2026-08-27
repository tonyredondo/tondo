# Native backend evaluation contract

`NATIVE-001` is an evidence and selection block. It does not choose a backend
until real candidate adapters have been measured against the same normalized
MIR shape and the VM oracle. The opt-in runner emits native executables for its
bounded supported slices, but this does not claim a complete Tondo product or
a selected backend. The machine-readable authority is
[`testing/native-evaluation.json`](../../testing/native-evaluation.json), and
the decision is recorded in
[`docs/adr/019-native-backend-selection.md`](../adr/019-native-backend-selection.md).
The short feedback loop is defined by
[`testing/native-evaluation-fast.json`](../../testing/native-evaluation-fast.json)
and [`scripts/native-evaluation-fast.sh`](../../scripts/native-evaluation-fast.sh).
The executable native differential lane is defined by
[`testing/native-evaluation-runner.json`](../../testing/native-evaluation-runner.json)
and [`scripts/native-evaluation-runner.sh`](../../scripts/native-evaluation-runner.sh).

## Decision boundary

Cranelift and LLVM are both candidates. The current evidence is intentionally
selection-pending: the fast lane measures their code-generation cost on the
same normalized module shape, but it cannot promote either one. A
compiler-owned generator is excluded from the measured ranking until it has a
real machine-code adapter; otherwise its result would be a made-up baseline,
not a candidate measurement.

The selection is deliberately narrower than Gate N1. The fast lane measures
compile time and object size only; it does not measure peak memory, runtime,
ABI, object layout, FFI, native memory policy or release readiness. Those
observations require native lowering and the VM oracle. A backend cannot enter
N1 if it loses an observable required by the VM or by the `DIAG-*` contracts.

## Real-MIR probe

The probe in
[`crates/tondo-compiler/examples/native_mir_probe.rs`](../../crates/tondo-compiler/examples/native_mir_probe.rs)
compiles hash-pinned `.to` fixtures through the ordinary `Operation::Run`
pipeline. It records a bounded `MirSummary`, the VM exit status, diagnostics
codes and a hash of stdout. In addition to the counters, the summary now
contains `tondo-mir-backend/1`: a path-free normalized instruction boundary
with ordinal locals/blocks, scalar operands, comparisons, checked arithmetic,
normal `goto`/`switch-bool`/tag-dispatch edges (including loop-carried locals), verified
direct calls to scalar functions, explicit trap operations and
unsupported-feature names.
It does not expose source paths,
addresses, pointers, layouts, process IDs, timestamps or ambient environment.

Each normalized program also carries the closed `tondo-mir-debug/1` metadata
from `NATIVE-LOWER-DEBUG-001`: logical source inventory, native symbol mapping,
function/block/statement/terminator source maps, unwind successors and stable
task/thread spawn identities. The adapter validates this metadata before either
candidate generates code; see [`native-lowering-debug.md`](native-lowering-debug.md).

The four fixtures cover the core/host boundary (including a pure scalar
operator matrix), collections and iteration, structured async (`await`,
`spawn`, task scopes and fallback paths), and the bytes slice. Their bytes are
checked before a report can be accepted. The VM is the semantic oracle: native
lowering must preserve values, errors, ordering, ownership, overflow,
cancellation and exit status before performance is compared. The common
adapter now lowers scalar assignments, comparisons, checked arithmetic and
logical operators, numeric conversions, verified direct calls, host/prelude
calls, opaque aggregate carriers, normal control flow (including loop-carried
locals), Option/Result tag dispatch, checked assertions, overflow,
invalid-shift and explicit-panic traps through the same normalized input in
Cranelift and LLVM. Awaited calls, eager `spawn`/`join`, cleanup metadata and
selection edges use the same operation/ABI boundary rather than a second
backend-specific representation.
The normalized boundary also exposes a checked zero-based bounds operation;
negative and past-the-end indices trap before a value is returned, with the
same policy in both backends and the oracle.
Read-only borrows of direct scalar locals are represented explicitly in the
adapter and lowered as value reads; projected borrows and any borrow that
would escape the scalar call boundary remain rejected rather than becoming a
native pointer by accident.
Managed `Option`/`Result` values use an opaque runtime result record. The
adapter lowers result construction, tag extraction and payload reads without
making a user-visible layout promise. Compiler-owned host operations use the
same runtime boundary; host state and handles never cross as addresses. The
probe's `load` fixture exercises both success and error result records and the
native runner compares their tags and payloads with the VM.
The adapter deliberately keeps storage/layout and scheduler details opaque.
Collection storage and `IteratorNext` are therefore retained as an explicit
fail-closed capability boundary: they are present in the normalized input but
not claimed as executable native semantics until their owning stdlib/native
ABI is selected. Source-level cleanup graphs and non-scalar calls have the
dedicated cleanup, ownership and structured-async contracts below; functions
outside the supported MIR/runtime slices are lowered to an explicit trap and
reported as `unsupported`, never silently approximated. This is a real adapter
boundary and backend-verifier check. The opt-in runner below extends it with
executable evidence without widening the source-level scalar MIR subset.

## Fast feedback lane

The fast lane intentionally uses one warmup and three measurements in one
process. It runs the MIR probe, builds the adapter harness once, and records
compile-time and code-size samples for Cranelift and LLVM. It may run on a
dirty worktree during local iteration. It never updates a reviewed baseline,
selects a backend or replaces the full test/coverage/mutation gate.

The adapter harness lives outside the workspace package graph in
`tools/native-evaluation/`, so ordinary compiler tests do not pay the cost of
the optional backend dependencies. It consumes only the probe JSON and an
explicit LLVM 18.x `llc` executable. Reports contain logical fixture
identities and tool versions, never physical tool paths. Every sample records
how many MIR functions were lowered by the scalar slice and how many were
rejected at the trap boundary.

## Executable native differential lane

The runner is an explicit, opt-in extension of the fast lane. It uses only
absolute `llc` and `cc` paths, emits a Cranelift object through
`cranelift-object`, emits an LLVM object through `llc`, links each with a
minimal C entry point and executes both binaries as subprocesses. The entry
point calls every supported scalar MIR function—including branch-return,
tag-dispatch and direct-call paths—
with deterministic nominal and boundary integer arguments and compares both
results or traps with the
normalized-MIR scalar interpreter and a direct bytecode-VM invocation of the
same function. The report records the function ordinal, arguments, expected
status, normalized result, VM status/result and diagnostics without retaining
temporary paths. This proves scalar value, direct-call ABI, branch joins,
tag-dispatch edges, loop-carried locals and exercised
checked-overflow/explicit-panic trap parity for the scalar slice. It also calls
supported managed-result functions and compares result discriminants and
payload carriers. Host-call lowering uses the same result-record path. The
dedicated runtime contracts below extend executable evidence to cleanup,
ownership and structured async state transitions without pretending that the
full source-level lowering is complete.
The normalized oracle has a fixed step budget, cyclic functions use deliberately
small deterministic inputs, and each native subprocess has a finite runtime
budget so an accidental infinite loop fails closed instead of hanging the
evaluation lane.

The runner also executes the private cleanup contract independently of source
fixture support: normal frame cleanup is idempotent (the second edge returns a
double-cleanup status) and an aborting frame leaves through the same terminal
transition. This keeps unwind/abort behavior observable while the full
source-level defer graph remains outside this scalar adapter slice.

The same runtime lane exercises ownership edges: a managed result is retained,
copy-on-written while shared, released to remove the original entry, and then
checked through the opaque result tag/payload API. The wrapper releases the
returned clone before exiting, so the case also checks the terminal ownership
path rather than only comparing a value.

It also executes the structured async runtime contract in fresh subprocesses:
pending tasks transition to ready through an explicit wake, `await` takes the
ready value, a scope joins its child task, scope cancellation propagates to
unfinished children, and polling observes the pending-to-ready transition.
An attempted wake after cancellation is rejected. Each case starts with a new
runtime table, so no task, scope or cancellation state can leak between cases.

The native selection slice is exercised in the same fresh-process runtime lane.
`select-begin`/register/commit/rollback are called through the private ABI and
compared with the VM selection observables: a ready borrowed `Join`, pending
wakeup, round-robin rotation, ownership-safe rollback, one-shot completion,
timer firing, thread join and `else`. The report exposes these eight cases in
`native_select_runs`; every case must pass in both Cranelift and LLVM and carry
the expected result. The contract also checks the three VM corpus cases in
`testing/async-select-conformance.json`, the 64-arm bound, one-lock
linearization, wakeup edges and the distinction between owned and borrowed
losers. The adapter never races tasks, polls a source or blocks a worker to
implement selection. Static and negative checks are in
[`scripts/native-select-check.sh`](../../scripts/native-select-check.sh) and
[`scripts/native-select-test.sh`](../../scripts/native-select-test.sh), with
the machine-readable boundary in
[`testing/native-select.json`](../../testing/native-select.json).

The native thread lane extends the same differential runner with a physical
worker check. `thread-spawn` is linked to a real `pthread_create`/`pthread_join`
worker in the C harness, and the private worker-status, run-count, distinct
thread and wait symbols are lowered through both adapters. Five cases prove
that a worker ran exactly once on a distinct OS thread, that `Join` observes the
value only after worker completion, and that cancellation leaves the logical
task cancelled. The Rust runtime implements the corresponding safe
`std::thread` signal and barrier; no operating-system thread ID or pointer is
serialized. The physical thread lane still evaluates its lowered scalar
operation before handoff. The minimum deferred direct-task coordinator is now
covered separately by `NATIVE-002`; mutable captures, closures, full scheduler
coordination and native storage remain explicit follow-ups.
The contract and focused checks are in
[`testing/native-thread.json`](../../testing/native-thread.json),
[`scripts/native-thread-check.sh`](../../scripts/native-thread-check.sh) and
[`scripts/native-thread-test.sh`](../../scripts/native-thread-test.sh).

## Evaluation matrix

The selection records the dimensions that must be evidenced before promotion:

| Dimension | Fast-lane outcome | Selection/N1 requirement |
| --- | --- | --- |
| MIR intake and correctness | Real-MIR probe passes on four fixtures | Full conformance corpus and exact VM oracle |
| Target support | Matrix remains a contract input | Pinned target descriptors and artifact identities |
| Compile latency and code size | Three samples per candidate | Full repeated capture with `PERF-001` |
| Runtime and peak memory | Deferred | Native executable plus repeated capture |
| Debugging, source maps, unwind | Required constraints recorded | Native frames and maps match diagnostic contracts |
| task/thread registry and memory/GC hooks | Required constraints recorded | Runtime ABI preserves observations |
| redaction and crash dumps | Required constraints recorded | `DIAG-NATIVE-001` passes fail-closed in both executable candidates |
| Distribution, maintenance, licensing | Decision recorded in ADR | Pinned, reviewable toolchain and supply-chain evidence |

The existing performance contract stays honest: no native entry is selected and
the full native capture remains deferred until complete lowering, the VM oracle
and the ARC/diagnostic gates are available. Fast-lane samples are exploratory
evidence and cannot be compared across targets, unpinned tools or incomplete
observables.

## Reproducibility and failure policy

Tool versions, backend dependencies and target descriptors are pinned before
lowering. Backend discovery through `PATH`, environment expansion, shell
injection or unhashed tool inputs is forbidden. The fast adapter receives an
explicit absolute `llc` path for execution, but never writes that path into the
report identity. Reports use logical fixture paths and content hashes only.
Missing fixtures, changed hashes, unknown features, an early selected
candidate, a premature N1/performance claim or a stale frontier is a hard
failure.

`DIAG-NATIVE-001` is closed by the native diagnostic section of the runner:
eight bounded cases execute through Cranelift and LLVM subprocesses, and each
backend must emit the same path-free envelope for race, leak/ARC and crash
profiles. The hosted diagnostic contracts are the oracle; unsupported physical
signal/register dimensions remain explicit target capabilities and are not
silently treated as passed.

`NATIVE-BACKEND-ADAPTER-001` is closed by the common normalized lowering and
its executable differential evidence. The report covers 118 scalar cases, 3
managed-result cases, 21 runtime-contract cases, 8 selection cases, 5
thread-worker cases and one deferred-task coordinator case in fresh
Cranelift/LLVM subprocesses. Functions that still require projected storage
(`core`/`bytes`) or collection `IteratorNext` are reported as unsupported,
fail-closed, and are not counted as native semantic evidence. Checked
arithmetic overflow, logical operators, conversions, comparison branches,
loop-carried locals, managed result records, host calls, structured async edges
and boundary trap parity for the exercised paths are covered by the executable
runner. Collection iteration, projected field/index storage and concrete
aggregate storage remain explicit follow-ups of the native stdlib/ABI
boundaries, not hidden approximations. ARC/diagnostic work may now consume the
coordinator contract, while backend selection and Gate N1 remain pending.

The static contract and negative cases run in the normal test gate. The
evaluation runner is opt-in/manual because it compiles the real fixture corpus;
it writes evidence below `target/reliability/evidence/` and never changes a
reviewed quality or performance baseline.
