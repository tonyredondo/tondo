# Native ARC implementation contract

`ARC-001` and `ARC-002` turn the hybrid memory decision into a runtime
implementation. The implementation is private to the native backend: handles
are opaque `u64` capabilities, the object layout is not an FFI promise, and
the VM remains the semantic oracle.

Both slices are closed for the unpublished 0.1 development line. Ownership,
terminal cleanup, cycle reclamation and weak-reference linearization are now
executable runtime contracts; native diagnostic parity is closed by
`DIAG-NATIVE-001` and the next native boundary is `NATIVE-STD-HOSTED-001`.

## Ownership and terminal cleanup

An unshared value starts with a checked non-atomic strong count. A value marked
as crossing a `Send`/`Share` boundary switches to a checked `AtomicU32`; retain
and release use a compare-update operation with acquire/release ordering and
fail closed on overflow or underflow. Every managed payload edge is retained
when it is published and is either transferred by a consuming operation or
released before the owner reaches its terminal state.

Frames publish roots before suspension and unpublish them on both normal and
abort cleanup. Structured scopes retain child tasks and cancel/discard their
payloads before releasing the scope edge. A select retains every registered
source; only an arm marked `owned` can cancel or discard that source. Thread
workers carry a runtime pin until their logical task is ready or cancelled, so
the physical worker cannot race object destruction.

No path invokes user finalizers. Resource cleanup remains the explicit MIR
terminal operation described by the language specification; ARC only releases
managed edges and drives the already-defined cancellation/unwind transitions.

## Cycles and weak references

At an explicit quiescence boundary or after 256 allocations, the runtime runs
trial deletion. It computes incoming strong edges, keeps components reachable
from strong owners, roots, or runtime pins, and atomically tombstones the
remaining components as a unit. Internal edges of a doomed component are not
temporarily decremented, which avoids underflow and preserves deterministic
teardown. A thread object in a collected component receives a cancellation
signal before its worker entry is discarded.

A weak handle retains only target tombstone metadata. It never contributes to
strong reachability. `weak_upgrade` is the single linearization point: it
increments the target strong count only while the target is alive, otherwise it
returns the `weak-dead` status and cannot resurrect the object. Concurrent
upgrades serialize at that point; every successful upgrade owns one strong
release. The tombstone is removed when the last weak handle is released.

Rooted cycles remain live through collection and are reclaimed only after their
last root/runtime pin is withdrawn. Detached cycles are reclaimed at explicit
quiescence and at the 256-allocation pressure threshold. The test corpus checks
both paths, concurrent live/dead upgrades, and fail-closed weak-handle misuse;
there are no user finalizers. The contract checker accepts the closed ARC
status, and the native diagnostic parity gate is now closed.

The machine-readable contract is
[`testing/native-arc.json`](../../testing/native-arc.json); its focused checker
and negative tests are `scripts/native-arc-{check,test}.sh`. Runtime evidence
is covered by the ARC-specific tests in
`crates/tondo-native-runtime/src/lib.rs`.
