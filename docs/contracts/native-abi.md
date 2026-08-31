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
to unfinished children. `task-complete` is the private lowering edge for a
deferred direct task call: it accepts only a pending handle, publishes the
value and wakes registered selections. Invalid transitions fail closed and
cannot be silently treated as a ready value.

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
it is not a public ABI or a production scheduler. `NATIVE-002` now coordinates
the minimum deferred direct-task slice; thread bodies still use the physical
worker lane and full scheduler/ownership integration remains a later gate.

Host handles are opaque capability-indexed values. The ABI does not expose a
pointer, object layout, allocator, symbol name, or FFI entry point to Tondo
source. A future public FFI would require a separate decision and versioned
contract.

## Synchronization host bridge

`STD-SYNC-HOST-001` adds a private, scalar synchronization lane without
changing that boundary. The runtime exports the following symbols:

- `tondo_rt_atomic_new`, `tondo_rt_atomic_load`, `tondo_rt_atomic_store`,
  `tondo_rt_atomic_swap` and `tondo_rt_atomic_compare_exchange` operate on an
  opaque `AtomicU64` cell. Memory-order codes are `0=Relaxed`, `1=Acquire`,
  `2=Release`, `3=AcqRel` and `4=SeqCst`; load/store/CAS-failure restrictions
  are checked before touching the cell. Compare-exchange is strong (no
  spurious failure) and reports a mismatch through the private status channel.
- `tondo_rt_sync_park_new`, `tondo_rt_sync_park_epoch`,
  `tondo_rt_sync_park_wait`, `tondo_rt_sync_park_wake` and
  `tondo_rt_sync_park_waiters` implement an epoch signal. A native worker may
  wait on a changed epoch with an optional nanosecond timeout; the epoch is
  advanced before notification, closing the check-then-sleep race. The
  cooperative VM never calls the blocking wait symbol: it keeps its waiter in
  scheduler state and polls/reacquires the resource instead.

The atomic and parking maps retain `Arc` cells outside the global handle-table
lock, so operations from distinct native workers are genuinely concurrent while
handle validation and retain/release remain serialized. Invalid handles,
invalid order pairs and timeout outcomes use the existing private status
channel. The same private bridge now carries the collection implementation
baseline. `tondo_rt_sync_array_*`, `tondo_rt_sync_map_*`, `tondo_rt_sync_set_*`,
`tondo_rt_sync_stack_*` and `tondo_rt_sync_queue_*` expose constructors,
observations, mutations, strong compare-exchange where defined and snapshots.
They accept and return opaque `u64` capabilities; arrays/maps/sets use
per-identity `RwLock` cells, stacks/queues use `Mutex` cells, and each cell has
an epoch parking signal. A native worker retries after a changed epoch and
never holds the global handle-table mutex while waiting. Release, copy-on-write
cloning, stale-handle rejection and cycle/terminal cleanup remove each backing
cell exactly once. The private CAS result tags are ABI-local and do not change
the public `Result`/`Option` types.

The direct-iteration leaf adds three private cursor symbols:
`tondo_rt_sync_cursor_start`, `tondo_rt_sync_cursor_next` and
`tondo_rt_sync_cursor_key`. A cursor retains its source collection through the
normal handle graph, captures only a finite structural horizon and selects
monotonic insertion generations under the collection's existing lock. The
`next` result carries one scalar value; a map key is read from the immediately
preceding successful step through `tondo_rt_sync_cursor_key`. Cursor state has
its own serialization, so a native worker never waits while holding the global
handle-table mutex. These symbols are private evidence for
`STD-SYNC-COLLECTION-ITER-001`, not a public FFI or a claim of generic AOT
lowering.

This collection lane is verified for the hosted VM and native runtime ABI only;
generic managed-value lowering, algorithmic lock-free fast paths and native AOT
lowering remain separate target-qualified work. The native runtime continues to
declare `#![forbid(unsafe_code)]`, and no pointer, layout or public FFI symbol is
introduced.

The machine-readable record is
[`testing/native-abi.json`](../../testing/native-abi.json). Its canonical typed
reader is in `crates/tondo-compiler/src/toolchain.rs`; static and negative
checks are in `scripts/native-abi-check.sh` and its focused test.
