# Direct `for` iteration contract for shared collections

`STD-SYNC-COLLECTION-ITER-001` closes the direct `for` boundary for the five
closed `std.sync` collection identities in the Tondo 0.1 draft. The
machine-readable record is
[`testing/stdlib-sync-collection-iter.json`](../../testing/stdlib-sync-collection-iter.json).
The collection construction and method contract remains
[`testing/stdlib-sync-collection.json`](../../testing/stdlib-sync-collection.json);
model/fuzz, conformance and public-documentation leaves remain separate
tracker tasks. The target-qualified hosted performance baseline is recorded in
the separate PERF leaf linked below.

This contract verifies the hosted VM and a private native-runtime cursor ABI.
Its target-qualified hosted performance baseline is recorded separately by
[`stdlib-sync-collection-performance.md`](./stdlib-sync-collection-performance.md).
It does not promote a source-level cursor type, expose a native layout, or
claim generic native AOT lowering.

## Closed source surface

Only these nominal identities select the intrinsic adaptation:

```tondo
std.sync.Array[T: Copy + Send + Share]
std.sync.Map[K: Key + Send + Share, V: Copy + Send + Share]
std.sync.Set[K: Key + Send + Share]
std.sync.Stack[T: Copy + Send + Share]
std.sync.Queue[T: Copy + Send + Share]
```

The ordinary header is the only source form:

```tondo
for value in values {
    use(value)
}
```

`for ref`, `for mut` and `for var` are rejected. No `for await`, `scan`,
`live`, global alias or second stream API is introduced. The source expression
is evaluated once and the compiler constructs an owning `cursor[sync,C]` with a
copy of the collection handle; it never copies collection contents or moves
the caller's handle.

`Stack` and `Queue` direct iteration additionally require `T: Copy + Send +
Share`. The traversal is observational: it never calls `pop` or `dequeue`.
`next` can suspend under collection contention, so a bodyful caller is promoted
to `suspends`; an explicit `@sync`/`@nosuspend` context receives `E1601`.

## Weak finite semantics

Cursor creation captures a finite structural horizon in O(1). It does not
retain a collection lock while the loop body executes and it does not allocate
or materialize storage proportional to collection cardinality.

- Array visits each fixed index once in ascending order. A replacement is read
  at that slot's `next` linearization point.
- Map and Set use insertion generations. A remove/reinsert receives a new
  generation and is outside an already-created cursor's horizon; a removal
  before `next` may therefore be omitted. Map yields `(key, value)` and Set
  yields the key/value respectively in insertion order.
- Stack selects existing generations from the top down; Queue selects them
  from the front forward. A push/enqueue after creation is excluded and the
  operation remains non-destructive.

Each structural generation is yielded at most once, including when writers
remove, reinsert or replace entries between calls. The cursor terminates after
the captured horizon even if writers continue adding values. `snapshot()` is
still the only one-linearization coherent materialization and is the required
choice for exact aggregation, equality or serialization.

## Runtime evidence

The hosted path keeps the collection as the source and stores only the cutoff,
last generation and direction in `IteratorAdapter::Sync`. The host's
generation metadata is updated at the same mutation points as the collection
value; `__iterStart` and `__iterNext` are private compiler-owned host calls.
The cooperative VM polls and parks through its scheduler and never calls a
blocking native wait.

The private native ABI exposes only opaque capability handles:

- `tondo_rt_sync_cursor_start(collection)` retains the source through a strong
  object edge and records the structural horizon without copying values.
- `tondo_rt_sync_cursor_next(cursor)` returns one opaque `Option` result. For a
  map, the value is returned and `tondo_rt_sync_cursor_key(cursor)` reads the
  key from the immediately preceding successful step.
- Per-collection `RwLock`/`Mutex` cells carry monotonic insertion generations;
  cursor state is separately serialized so a contended worker never waits
  while holding the global handle-table mutex.

Releasing a cursor removes its state and releases the source edge. Wrong,
stale or forged handles fail closed. The ABI is `#![forbid(unsafe_code)]` and
remains private; generic managed-value lowering and native AOT are not claimed.

## Evidence and promotion

The checker regression covers value-only bindings, inferred suspension and
stack/queue capability diagnostics. The hosted end-to-end test covers all five
orders and exact finite termination. The native runtime test covers array
replacement, map/set removal and reinsertion, stack/queue boundaries, source
retention, stale handles and cleanup. Contract and negative-case runners are
`scripts/stdlib-sync-collection-iter-check.sh` and
`scripts/stdlib-sync-collection-iter-test.sh`; both are integrated into the
standard test gate.

This leaf does not close cross-target conformance or the broader `std.sync`
documentation task. The hosted performance measurement is closed by its
separate target-qualified contract; native algorithm selection and generic AOT
remain unclaimed. The remaining leaves stay visible in the JSON record and
tracker.
