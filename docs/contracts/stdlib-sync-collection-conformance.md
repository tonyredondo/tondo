# std.sync collection VM/native conformance

This contract closes `STD-SYNC-COLLECTION-CONF-001`. It compares the hosted VM
fixture with a fresh-process execution of the private native collection ABI
using the same eight logical cases and the same ordered observable lines. The
machine-readable authority is
`testing/stdlib-sync-collection-conformance.json`.

The fixture and probe use the same observable lines for every shared case.

The corpus covers qualified literals and copy-handle aliases, fixed-array
bounds and strong compare-exchange outcomes, insertion-linearized Map/Set
replacement and reinsertion, non-destructive Stack/Queue LIFO/FIFO order,
finite direct cursors, coherent snapshots, empty and oversized operations,
stale/wrong handles, and cross-thread sharing under the explicit `threads`
capability. The hosted fixture also exercises the ordinary value-only `for`
surface and emits one line per case. The native probe inspects the opaque
Option/Result tags, keys, values, generation horizon and cleanup rather than
accepting the lines as a hard-coded oracle.

Direct `for` is the inferred suspendable `AsyncIterator` view: bindings remain
by value, stack and queue iteration never consumes their source, and the
cursor has a finite structural horizon. Existing compiler and hosted-runtime
tests are part of the conformance lane for rejected `ref`/`mut`/`var` bindings,
missing `Copy + Send + Share` bounds, suspension inference, and removal/
reinsertion generation behavior. The contract therefore checks both executable
observables and the static boundary.

The native run is ABI evidence only. It validates the current host-target
runtime implementation and its cleanup; it does not claim that generic AOT
lowering, native AOT fast paths, or a public cursor API have shipped. Cooperative
hosted execution remains non-blocking, while the native probe may use OS
workers for the `threads` case. Every case starts from a fresh runtime state,
releases all results, snapshots and cursors, and requires zero live objects
before it emits its line. Reports contain hashes and normalized observations,
never physical paths, addresses, process IDs or timestamps.

Run the lane with:

    scripts/stdlib-sync-collection-conformance.sh

The runner writes
`target/reliability/evidence/stdlib-sync-collection-conformance.json` with the
fixture/probe hashes, exact VM lines, native observations and cleanup/static
comparison flags. Contract mutations are exercised by
`scripts/stdlib-sync-collection-conformance-test.sh`.
