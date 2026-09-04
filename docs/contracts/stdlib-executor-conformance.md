# `std.executor` VM/native conformance

This contract closes `STD-EXEC-CONF-001`. The machine-readable authority is
[`testing/stdlib-executor-conformance.json`](../../testing/stdlib-executor-conformance.json).
It replays one eight-case observable corpus on the hosted VM and the private
native executor bridge. The VM fixture runs through a temporary project whose
manifest declares `threads`; its repository fixture also records the same
declaration in `m11-std-executor-conformance-001.capabilities` so no standalone
source invocation receives an ambient capability.

The shared cases cover bounded pool admission and saturation, blocking result
transfer, safe cancellation and drain, actor FIFO and terminal error behavior,
the explicit `threads` capability, and the native AOT boundary. The VM emits
one exact line per case followed by `executor-conformance-ok`. The native probe
emits the same case IDs with normalized result tags. Cases without a native
public ABI are marked `delegated` explicitly; they are not silent stubs or
claims of native actor support.

`blockingPool` is rejected statically with `E1008` when the target does not
declare `threads`. The compile-fail fixture and the driver test are part of the
same gate. The native lane is target-qualified to `x86_64-unknown-linux-gnu`,
uses opaque tokens, and checks lifecycle, worker identity, managed payload
transfer, cancellation and cleanup. It does not expose callbacks, pointers or
an ABI layout.

Every native case starts from a fresh runtime state and requires zero live
handles before its result. Reports contain source and input hashes but no
physical paths, addresses, process IDs or timestamps. The comparison keeps the
hosted cooperative pool and actor observations separate from the private
blocking-token lane.

Run the lane with:

    scripts/stdlib-executor-conformance.sh

Contract mutations are exercised by:

    scripts/stdlib-executor-conformance-test.sh

The runner writes
`target/reliability/evidence/stdlib-executor-conformance.json` with the exact VM
lines, native observations, static capability result, cleanup checks and the
hosted/native/AOT boundary. Native AOT callable lowering remains
`not-claimed`.
