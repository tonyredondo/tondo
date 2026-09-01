# Shared collection performance contract

`STD-SYNC-COLLECTION-PERF-001` closes the target-qualified performance
measurement boundary for the five `std.sync` collection owners in the Tondo
0.1 draft. The machine-readable contract is
[`testing/stdlib-sync-collection-performance.json`](../../testing/stdlib-sync-collection-performance.json).

This report measures the real hosted VM baseline (`tondo-vm-hosted`,
`bytecode-vm`). The hosted implementation has one scheduler-owned worker, so
the report must not be read as native MPMC throughput or as proof of a
lock-free algorithm. Native runtime contention, AOT lowering and final
algorithm selection remain target-qualified follow-up work. In particular,
`native AOT` is not claimed by this contract.

## Protocol and identity

The private probe is
`process_host::tests::sync_collection_performance_probe` in
[`crates/tondo-compiler/src/process_host.rs`](../../crates/tondo-compiler/src/process_host.rs).
It runs three warmups and nine measured repetitions in each of three
independent processes, for 27 samples per workload. The monotonic clock,
16-operation batch and deterministic seed are fixed in the JSON contract.
Outliers remain in `samples_ns`; they are never discarded to improve a
percentile. The report identity is limited to the declared suite, workload,
probe hash, target, backend, profile, toolchain, flags and git revision. PID,
path, timestamp, ambient environment and instantaneous CPU frequency are
forbidden identity inputs.

Each sample reports:

- latency and derived throughput, including median, P95 and P99;
- logical host-value allocations, live handles and peak logical memory;
- retries, wakeups and parking events;
- collection, mode, participant count and cardinality.

Logical memory includes collection payload capacity, generation metadata and
the host registry entry. It excludes allocator headers, fragmentation and RSS.
Allocation counts are host-value identities created by the fixture and are not
OS allocation counts.

## Workload coverage

The 31 workloads cover the requested cardinality/participant dimensions and
the observable operation families:

| Owner | Workloads | Boundary exercised |
| --- | --- | --- |
| `Array` | one-to-one, read-dominant, independent writes, hot slot, cursor, snapshot | fixed slots, read/write fast paths and direct `for` |
| `Map` | independent/hot-key reads and writes, resize, cursor, snapshot | key locality, growth and insertion order |
| `Set` | independent/hot-key reads, writes, resize, cursor, snapshot | membership locality, growth and insertion order |
| `Stack` | MPMC-style push/pop, resize, cursor, snapshot | LIFO observation and bounded growth |
| `Queue` | MPMC-style enqueue/dequeue, resize, cursor, snapshot | FIFO observation and bounded growth |

“MPMC-style” here means deterministic logical execution units driving the
same hosted owner; it does not create native threads. Counters for retries,
wakeups and parking are therefore required to be zero in this hosted report,
which prevents accidentally combining cooperative scheduler evidence with a
native contention claim.

## Oracles and gates

The runner first executes the independent bounded model in
[`crates/tondo-reliability/src/sync_collection_model.rs`](../../crates/tondo-reliability/src/sync_collection_model.rs)
and its tests. The probe then checks every operation's observable result,
bounded growth, cursor termination, snapshot shape and scheduler cleanup.
The aggregation rejects missing or duplicate workloads, unstable operation
counts, invalid sample cardinalities, non-zero hosted contention counters,
pending jobs/waiters, and mixed target/backend reports.

The `direct-next-has-no-content-materialization-or-visited-table` invariant is
explicit. Cursor setup captures only the structural cutoff; it does not call
`snapshot`, allocate a visited table or retain a lock across the loop body.
Snapshots remain the only coherent materialization operation. The checker also
keeps the implementation contract's
`algorithmic_fast_paths` boundary at
`deferred-to-STD-SYNC-COLLECTION-PERF-001` and refuses an unsubstantiated
lock-free or AOT claim.

## Strategy decision

For this target the selected strategy is
`single-worker-ready-job-collection-baseline`. It is the measured correctness
carrier already used by the hosted VM. Native strategy selection is deliberately
deferred until a comparable concurrent campaign can measure lock-free reads,
slot/key locality, Treiber/Michael--Scott candidates, resize, reclamation,
retries, wakeups and parking on the native ABI. No semantic result, ordering,
ownership rule or public API changes with that future choice.

## Reproduction

```bash
scripts/stdlib-sync-collection-performance-check.sh
scripts/stdlib-sync-collection-performance-test.sh
TONDO_STDLIB_SYNC_COLLECTION_PERF_ALLOW_DIRTY=1 \
  scripts/stdlib-sync-collection-performance.sh
```

The runner writes
`target/reliability/evidence/stdlib-sync-collection-performance.json` and
rejects a dirty workspace by default. CI executes it on a clean checkout.
The native runtime and generic AOT remain separate evidence lanes; this
report never upgrades either one implicitly.
