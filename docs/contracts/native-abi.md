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

The structured async lowering uses opaque runtime handles and one status
machine in both native adapters. `scope-enter` creates a scope;
`scope-spawn` attaches a pending or ready child; `task-spawn` creates an
unscoped child; `task-poll` observes pending, ready, cancelled or joined;
`task-wake` performs the pending-to-ready transition; `await`/`task-take`
consume a ready value; `scope-join` consumes a ready child belonging to an
open scope; and `scope-cancel` closes the scope while propagating cancellation
to unfinished children. Invalid transitions fail closed and cannot be
silently treated as a ready value.

Host handles are opaque capability-indexed values. The ABI does not expose a
pointer, object layout, allocator, symbol name, or FFI entry point to Tondo
source. A future public FFI would require a separate decision and versioned
contract.

The machine-readable record is
[`testing/native-abi.json`](../../testing/native-abi.json). Its canonical typed
reader is in `crates/tondo-compiler/src/toolchain.rs`; static and negative
checks are in `scripts/native-abi-check.sh` and its focused test.
