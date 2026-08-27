# Native lowering coordinator

`NATIVE-002` is the executable coordination gate between the verified MIR
adapter slices and the later native memory/stdlib work. It does not select
Cranelift or LLVM, expose a public ABI, or claim Gate N1. It proves that one
normalized MIR program can be accepted by both candidate adapters while the
individual lowering contracts share one source of truth.

## One coordinator, one boundary

The coordinator consumes `tondo-mir-backend/1` together with its required
`tondo-mir-debug/1` metadata. It validates the program once, then sends that
same immutable shape to Cranelift and LLVM. Unsupported storage, captures,
closures and protocols remain explicit fail-closed capabilities; the
coordinator never rewrites them into a guessed native representation.

The minimum deferred callable slice is deliberately narrow and useful:

* `spawn call()` for a direct task call publishes an opaque `Pending` handle
  before evaluating the callable body;
* the first `Join`/`await` evaluates the direct call exactly once, commits the
  value through `tondo_rt_task_complete`, and consumes it through the ordinary
  `tondo_rt_await` transition;
* only immutable scalar constants are captured in this first slice. Mutable
  captures, closures and indirect calls stay unsupported until their native
  storage/ownership ABI is closed;
* `spawn thread call()` keeps the physical worker lane and completion barrier
  from `NATIVE-THREAD-001`; this coordinator does not replace an OS worker with
  parent-thread execution.

The runtime transition is private and versioned. User code still sees one
`Join` carrier and one inferred `await`; no synchronous/async API pair or
native pointer is introduced.

## Coordinated slices

The gate requires executable evidence for all of these already-closed slices:

| Slice | Required evidence |
| --- | --- |
| calls | direct-call ABI, managed carriers and host results |
| control | branches, loops, checked operations and traps |
| cleanup | normal/abort terminal edges and exact-once cleanup |
| ownership | retain/release and uniqueness-guarded COW |
| async | task poll/wake/await, structured scope and cancellation |
| select | prepare/register/commit/rollback, fairness and wakeups |
| thread | OS worker, join barrier and cancellation |
| debug | symbols, logical source maps, unwind and task/thread identity |
| deferred | pending-before-join, one completion and joined-after-join |

Every slice is run against both candidate adapters in fresh subprocesses. The
report records logical case IDs and statuses only; temporary paths, addresses,
process IDs and host environment never enter the report.

## Safety and lifecycle rules

`task_complete` accepts only a pending task. A second completion, completion of
a ready/ joined/cancelled task, or an invalid handle is rejected. `await` is
the only consumer of the completed value, so the existing affine ownership and
selection rules remain the single source of truth. Each native case starts a
fresh runtime table and must finish within the evaluator's finite budget.

The coordinator is a proof of the minimum lowering path, not a performance
baseline. Runtime, peak-memory, full collection storage, ARC cycle collection,
stdlib-hosted capabilities and diagnostic parity remain later N1/S1 gates.

The machine-readable authority is
[`testing/native-lowering.json`](../../testing/native-lowering.json). Static
and negative checks are `scripts/native-lowering-{check,test}.sh`; executable
evidence is the `native_lowering_runs` field in
`target/reliability/evidence/native-evaluation-runner.json`.
