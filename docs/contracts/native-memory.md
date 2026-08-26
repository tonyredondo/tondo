# Native memory contract

`NATIVE-MEM-ADR-001` closes the memory policy that the private native runtime
must preserve. It does not expose an object layout, a stable symbol, or a
foreign-function interface. The bytecode VM remains the semantic oracle; this
record fixes the ownership and liveness invariants that a native lowering must
implement.

## Decision

Tondo's native implementation uses the `hybrid-arc-cycle-collector` policy:

- unshared managed values use non-atomic strong counts;
- values that can cross `Send`/`Share` boundaries use atomic strong counts;
- a trial-deletion cycle pass runs at quiescence and under allocation pressure;
- weak edges are runtime-managed and never participate in strong liveness;
- stack, task, thread, async-frame and host-handle registries publish roots;
- affine resources are released by verified MIR cleanup, never by the
  collector;
- cancellation drains cleanup before a task becomes terminal;
- copy-on-write is permitted only after a uniqueness check and is not observable;
- the physical representation remains `private-versioned-no-ffi-promise`.

This is deliberately a hybrid rather than pure reference counting: a cycle
that loses all roots must be reclaimable without requiring the user to insert a
weak reference. The collector never runs user finalizers and never replaces the
deterministic terminal operation of a file, process, lock, or other resource.

## Root and lifecycle invariant

Every native frame publishes its managed roots before a suspension point or a
host call can park it. A move, retain, release, cancellation edge and resource
terminal operation is emitted by MIR and is checked exactly once. A missing
root, underflow, double terminal operation, or impossible ownership transition
is a fail-closed runtime diagnostic, not an inferred success.

The machine-readable record is
[`testing/native-memory.json`](../../testing/native-memory.json). Its typed
reader and canonical encoding live in
`crates/tondo-compiler/src/toolchain.rs`; the shell contract is checked by
`scripts/native-memory-check.sh` and its focused test.
