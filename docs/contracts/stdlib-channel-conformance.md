# `std.channel` VM/native conformance

This contract closes `STD-CHANNEL-CONF-001`. The machine-readable authority is
[`testing/stdlib-channel-conformance.json`](../../testing/stdlib-channel-conformance.json).
It replays one eight-case observable corpus on the hosted VM and the private
native channel ABI, plus a separate panic fixture. Each probe is a fresh
process and every case must finish with no live endpoint or waiter.

The private native channel ABI is the target-qualified native surface for this
leaf.

The shared cases cover bounded FIFO commit and `Full(value)`, rendezvous
wakeup, terminal receiver drainage, closed-send payload preservation, invalid
and resource-limited capacities, the hosted `select` commit path, close
wakeup of a blocked sender, and deferred cleanup before panic propagation. The
VM emits one exact line per case. The native probe emits the same case IDs with
normalized result tags and status fields; values are integers, so no native
layout is inferred from the comparison.

`select-commit` is deliberately target-qualified. The hosted fixture executes
the core `select` expression, while the current native channel bridge exposes
only opaque endpoint operations and therefore records the private ABI boundary
instead of claiming a native select API. The existing hosted implementation
and scheduler tests remain the proof for prepare/commit/rollback semantics.

Errors are observable, not swallowed: negative capacity reports
`ChannelError.InvalidCapacity`, a full bounded queue returns `Full(value)`, a
closed sender retains `value`, and a resource-sized capacity is rejected. A
blocked send is woken by the last receiver close with its payload intact. The
panic fixture uses `defer cleanup_receiver(receiver)` before `panic`; its exit code and
cleanup marker are checked independently. The native probe uses a small
unwind guard around the same endpoint lifecycle and requires zero live objects
after the panic is caught.

The native run is ABI evidence on the host target only. It does not claim
native AOT or Cranelift lowering, native algorithmic fast paths, a public FFI layout, or
public promotion of `std.channel`. Reports contain source and input hashes but
no physical paths, addresses, process IDs, or timestamps.

Run the lane with:

    scripts/stdlib-channel-conformance.sh

Contract mutations are exercised by:

    scripts/stdlib-channel-conformance-test.sh

The runner writes
`target/reliability/evidence/stdlib-channel-conformance.json` with the exact VM
lines, panic result, native case observations, cleanup checks, and the
hosted/native/AOT boundary.
