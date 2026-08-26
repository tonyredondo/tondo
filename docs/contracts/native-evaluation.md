# Native backend evaluation contract

`NATIVE-001` is an evidence and selection block. It does not choose a backend
until real candidate adapters have been measured against the same normalized
MIR shape and the VM oracle. It does not claim that Tondo already emits a
native executable. The machine-readable authority is
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

The four fixtures cover the core/host boundary (including a pure scalar
arithmetic matrix), collections and iteration, structured async (`await`,
`spawn`, task scopes and fallback paths), and the bytes slice. Their bytes are
checked before a report can be accepted. The VM is
the semantic oracle: native lowering must preserve values, errors, ordering,
ownership, overflow, cancellation and exit status before performance is
compared. The first adapter slice lowers `Int` scalar assignments,
comparisons, checked arithmetic, verified direct scalar calls and scalar normal
control flow, including loop-carried locals, Option/Result tag dispatch,
checked assertions, overflow, invalid-shift and explicit-panic traps, through
the same normalized input in Cranelift and LLVM.
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
Cleanup edges and non-scalar calls remain outside the contract. Functions
outside that slice are lowered to an explicit trap and reported as
`unsupported`; they are never silently approximated. This is a real adapter
boundary and backend-verifier check. The opt-in scalar runner below extends it
with executable evidence without widening the supported MIR subset.

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
payload carriers. Host-call lowering uses the same result-record path. Cleanup
and async MIR families remain open for their dedicated runtime contracts.
The normalized oracle has a fixed step budget, cyclic functions use deliberately
small deterministic inputs, and each native subprocess has a finite runtime
budget so an accidental infinite loop fails closed instead of hanging the
evaluation lane.

The runner also executes the private cleanup contract independently of source
fixture support: normal frame cleanup is idempotent (the second edge returns a
double-cleanup status) and an aborting frame leaves through the same terminal
transition. This keeps unwind/abort behavior observable while the full
source-level defer graph is lowered.

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
| redaction and crash dumps | Required constraints recorded | `DIAG-NATIVE-001` passes fail-closed |
| Distribution, maintenance, licensing | Decision recorded in ADR | Pinned, reviewable toolchain and supply-chain evidence |

The existing performance contract stays honest: no native entry is selected and
the full native capture remains deferred until lowering and the VM oracle are
available. Fast-lane samples are exploratory evidence and cannot be compared
across targets, unpinned tools or incomplete observables.

## Reproducibility and failure policy

Tool versions, backend dependencies and target descriptors are pinned before
lowering. Backend discovery through `PATH`, environment expansion, shell
injection or unhashed tool inputs is forbidden. The fast adapter receives an
explicit absolute `llc` path for execution, but never writes that path into the
report identity. Reports use logical fixture paths and content hashes only.
Missing fixtures, changed hashes, unknown features, an early selected
candidate, a premature N1/performance claim or a stale frontier is a hard
failure.

`NATIVE-BACKEND-ADAPTER-001` remains open until the adapter extends this
scalar/managed-CFG VM differential to the remaining MIR families, with explicit
diagnostics for unsupported paths. Checked arithmetic overflow, comparison
branches, loop-carried locals, managed result records, host calls and boundary
trap parity for the exercised paths are covered by the executable runner; the
other helper families are compile- and oracle-tested. Only after that evidence
may memory/ABI work consume a backend selection.

The static contract and negative cases run in the normal test gate. The
evaluation runner is opt-in/manual because it compiles the real fixture corpus;
it writes evidence below `target/reliability/evidence/` and never changes a
reviewed quality or performance baseline.
