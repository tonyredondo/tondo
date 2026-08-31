# Shared collection implementation contract

`STD-SYNC-COLLECTION-IMPL-001` closes the executable collection boundary for
the Tondo 0.1 draft. The machine-readable record is
[`testing/stdlib-sync-collection.json`](../../testing/stdlib-sync-collection.json).
The syntax and nominal-resolution prerequisite remains
[`testing/stdlib-sync-collection-frontend.json`](../../testing/stdlib-sync-collection-frontend.json),
and the parent owner contract is
[`testing/stdlib-sync.json`](../../testing/stdlib-sync.json).

This contract verifies the hosted VM and the private native runtime ABI. It does
not promote a public release API, expose a native layout, or claim generic
native AOT lowering. Direct concurrent iteration is the next independent leaf:
`STD-SYNC-COLLECTION-ITER-001`.

## Implemented surface

The five closed nominal identities are:

```tondo
std.sync.Array[T: Copy + Send + Share]
std.sync.Map[K: Key + Send + Share, V: Copy + Send + Share]
std.sync.Set[K: Key + Send + Share]
std.sync.Stack[T: Send + Discard]
std.sync.Queue[T: Send + Discard]
```

Their handles are `Copy + Discard + Send + Share`. Copying a handle copies the
identity, not the contents. The hosted checker, HIR, MIR, bytecode verifier and
host all retain the nominal identity; a user type or forged spelling cannot
select these operations.

The hosted implementation supports the five literal constructors and the
closed method set: length, emptiness, indexed array access and replacement,
strong compare-exchange, map/set membership and mutation, stack push/pop/peek,
queue enqueue/dequeue/peek, and snapshots. Array length is fixed. Map and set
preserve insertion order at the operation's linearization point; replacement
does not move a key, while remove/reinsert appends it. Stack is LIFO and queue
is FIFO-MPMC. Empty pop/dequeue return `none`; no hidden wait operation exists.

Every individual operation is linearizable. `compareExchange` is strong and
returns the observed value on mismatch without writing. `snapshot` copies one
coherent value-collection at one linearization point. The hosted model is
`single-worker-ready-job-linearization`: it uses one scheduler-owned worker and
completes collection jobs through the common ready-job path; it does not
simulate native thread contention.

## Native runtime ABI

The native lane is private and scalar-carrier based. Its carrier is the
`opaque-u64-capability`: collection values cross the boundary only as opaque
monotonic `u64` capabilities. The runtime keeps
the actual value in an `Arc` cell selected after handle and object-kind
validation:

- arrays, maps and sets use per-identity RwLock cells;
- stacks and queues use per-identity Mutex cells;
- each identity has an epoch `Condvar` parking signal;
- native workers retry after a changed epoch and never hold the global handle
  table mutex while waiting;
- `Option`/`Result` outcomes use opaque runtime records, while private CAS tags
  distinguish `Exchanged` from `Mismatch` without racing on the diagnostic
  status channel.

Release, copy-on-write cloning, stale-handle rejection and cycle/terminal
cleanup remove the corresponding value and parking cells exactly once. The
native runtime remains `#![forbid(unsafe_code)]`; no pointer or object layout
is part of this contract. Blocking is permitted only on native workers. The
cooperative VM never calls the native blocking wait symbol.

The current cell carriers are the correctness baseline for this block. Treiber
stack, Michael--Scott queue, slot-level CAS, sharding and other algorithmic
fast paths are performance work tracked by `STD-SYNC-COLLECTION-PERF-001`; they
are not claimed by this contract. The same applies to generic native AOT
lowering.

## Bounds and failures

The private runtime uses the bounded `HOST_MAX_BYTES` budget as a conservative
logical element/entry limit. Constructors and growth that exceed the budget
fail recoverably. Invalid array indices produce `CollectionError` (or the
private status equivalent), and invalid or stale capabilities fail closed
before any collection cell is accessed. No operation creates an unbounded
queue or hides backpressure in `std.sync`.

## Evidence and promotion

The focused implementation tests cover nominal method registration, hosted
execution, ordering and limits, forged/stale tokens, native strong CAS,
shared-handle state, MPMC queue progress, worker cleanup and the full native
runtime suite. The check and negative-contract runner are
`scripts/stdlib-sync-collection-check.sh` and
`scripts/stdlib-sync-collection-test.sh`; both are integrated into
`scripts/test-gate.sh`.

This leaf is complete for the hosted VM and native ABI only. It does not close
the direct `for` cursor, model/fuzz campaign, performance campaign, cross-target
conformance, or documentation leaf. Those remain visible in the tracker and in
the `promotion.remaining` list of the JSON record.
