# Native `Thread` lane

`NATIVE-THREAD-001` closes the physical worker boundary for the private native
runtime. It does not add a public threading API, expose an operating-system
thread identifier, or select Cranelift/LLVM. Tondo keeps one source shape:
`spawn call()` uses the cooperative task lane and `spawn thread call()` uses an
OS-worker lane while both return the same affine `Join` carrier.

## One state machine, two execution lanes

The hosted VM remains deterministic and cooperative. Its `Thread` handle uses
the same pending/ready/cancelled/joined state machine as a task. The native
runtime allocates an opaque handle and a private worker signal, then starts one
`std::thread` worker for that handle. The signal records only logical state,
the number of worker entries and whether the worker ran on a distinct thread;
physical IDs, addresses and paths never enter the ABI.

`Join`, `await` and the winning `select-take` cross the worker-completion
barrier before consuming the value. A normal task still follows the existing
non-blocking state transitions. Cancelling a thread handle transitions the
logical handle to `cancelled`; a cancelled join never returns its value. The
worker signal is retained until the logical handle is released and its
terminal transition is exact-once, so a detached implementation detail cannot
leak a runtime entry.

The private verification symbols are:

| Symbol | Result |
| --- | --- |
| `tondo_rt_thread_worker_status(task)` | `starting=0`, `running=1`, `completed=2`, `cancelled=3`; invalid/non-thread is `u64::MAX` |
| `tondo_rt_thread_worker_runs(task)` | worker-entry count; invalid/non-thread is `u64::MAX` |
| `tondo_rt_thread_worker_distinct(task)` | `1` when the worker differs from its spawner, `0` otherwise; invalid is `u64::MAX` |
| `tondo_rt_thread_worker_wait(task)` | waits without consuming `Join`; returns the normal runtime status or `cancelled` |

These are compiler/runtime evidence hooks, not source-level functions.

## Native adapter boundary

The current MIR adapter lowers a `Spawn` operation eagerly: its scalar value
is computed before the thread handoff. The native runner nevertheless links a
real `pthread_create`/`pthread_join` worker and verifies status, run count,
distinct-thread identity, join value and cancellation in both Cranelift and
LLVM subprocesses. This is deliberately explicit: the block proves the
physical worker lane and join barrier without claiming that an eager adapter
has deferred callable-body lowering. Coordinating that deferred body and the
full native scheduler is `NATIVE-002`.

The C implementation is a deterministic differential harness used only by the
opt-in evaluator. It is not the production scheduler and is never a public C
ABI. The Rust runtime is the safe implementation used for runtime unit tests;
its worker entry contains no unsafe code and never holds the global runtime
mutex while waiting for a worker.

The machine-readable contract is
[`testing/native-thread.json`](../../testing/native-thread.json). Static and
negative checks are `scripts/native-thread-check.sh` and
`scripts/native-thread-test.sh`; executable evidence is the
`native_thread_runs` field in
`target/reliability/evidence/native-evaluation-runner.json`.
