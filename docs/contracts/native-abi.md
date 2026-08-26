# Private native runtime ABI

`NATIVE-ABI-001` defines the compiler/runtime boundary used by the first native
backend. It is an internal, versioned contract. It is not a C ABI, a plugin
ABI, a user type-layout guarantee, or a promise about stable mangled names.

## Boundary

The ABI has one `verified-ordinal-resolved-private-symbols` direct-call path for scalar lowering and one runtime
result path for managed values and failures. Direct calls are resolved from
verified private function ordinals; indirect calls, unsupported protocols and
unknown targets trap before code generation. The runtime result record carries
normal value/error state without duplicating public sync/async APIs.

Ownership edges are explicit in MIR: retains/releases for managed values,
terminal cleanup for affine resources, and root publication for stack, task,
thread, async-frame and host-handle owners. Unwind is explicit normal-unwind or
abort; cancellation is a cleanup edge, not a hidden destructor. Async frames
register with the task/waker registry before suspension and carry source-span,
task/thread and crash-envelope identities for diagnostics.

The concrete MIR-to-native identity transport is `tondo-mir-debug/1`, closed by
`NATIVE-LOWER-DEBUG-001`: logical source ordinals and hashes, source-map region
IDs, canonical native symbols, unwind successors and task/thread execution IDs.
It is validated before code generation and does not expose physical paths or
object addresses.

The structured async lowering uses opaque runtime handles and one status
machine in both native adapters. `scope-enter` creates a scope;
`scope-spawn` attaches a pending or ready child; `task-spawn` creates an
unscoped child; `task-poll` observes pending, ready, cancelled or joined;
`task-wake` performs the pending-to-ready transition; `await`/`task-take`
consume a ready value; `scope-join` consumes a ready child belonging to an
open scope; and `scope-cancel` closes the scope while propagating cancellation
to unfinished children. Invalid transitions fail closed and cannot be
silently treated as a ready value.

## Atomic selection

The native selector is the same prepare/register/commit/rollback machine as the
hosted VM. `select-begin` reserves a selection with at most 64 arms;
`select-register-task`, `select-register-join`,
`select-register-oneshot` and `select-register-time` add the only permitted
source kinds. Registration is closed once the selection waits or commits.
`select-commit` linearizes one round-robin scan while holding the runtime state
lock. A ready source commits exactly one winner, advances the process-local
rotation and discards owned losers. A pending selection returns `not-ready`
without polling or blocking a worker, keeps its registrations live and is woken
by task, one-shot or timer transitions. An `else` commit returns the dedicated
`select-else` status and rolls back owned arms. `select-take` consumes the
winning source exactly once; borrowed `Join` arms remain caller-owned, including
when they lose. `select-rollback` is valid only before a commit and releases
owned sources without touching borrowed ones. Invalid capacity, duplicate arms,
phase transitions and source kinds fail closed.

Threads use the same join state machine as tasks, with a private worker signal
retaining the logical handle until its terminal release. `Join`, `await` and a
winning `select-take` cross that worker-completion barrier before consuming the
value. The verification-only worker symbols expose lifecycle status, run count,
distinct-thread identity and a non-consuming wait; they never expose an OS
thread ID or pointer. One-shots have exactly one completion and timers are
scheduler fire transitions; neither adapter introduces a second async API. The
Rust runtime's mutex is the atomic linearization boundary and its worker uses a
safe `std::thread` entry. The native evaluator's C shim uses
`pthread_create`/`pthread_join` only as a deterministic differential harness;
it is not a public ABI or a production scheduler. The current adapter hands an
already-lowered value to the worker lane; deferred callable-body lowering is
reserved for `NATIVE-002`.

Host handles are opaque capability-indexed values. The ABI does not expose a
pointer, object layout, allocator, symbol name, or FFI entry point to Tondo
source. A future public FFI would require a separate decision and versioned
contract.

The machine-readable record is
[`testing/native-abi.json`](../../testing/native-abi.json). Its canonical typed
reader is in `crates/tondo-compiler/src/toolchain.rs`; static and negative
checks are in `scripts/native-abi-check.sh` and its focused test.
