# Shared collection model and fuzz contract

`STD-SYNC-COLLECTION-TEST-001` closes the bounded model, history and fuzz
campaign for the five `std.sync` collection identities in the Tondo 0.1 draft.
The machine-readable record is
[`testing/stdlib-sync-collection-test.json`](../../testing/stdlib-sync-collection-test.json).
The runtime surface and private hosted/native implementation remain in
[`stdlib-sync-collection.md`](./stdlib-sync-collection.md), while direct `for`
semantics remain in [`stdlib-sync-collection-iter.md`](./stdlib-sync-collection-iter.md).

This is a reliability boundary, not a new runtime or public API. The model is
independent of the VM and native cells, and the fuzz target exercises the model
only. Existing hosted and native regression suites are rerun to guard the
already-implemented boundary. No generic native AOT lowering, public cursor API
or algorithmic performance promotion is implied.

## Sequential and history oracle

`sync_collection_model.rs` contains ordinary Rust states for `Array`, `Map`,
`Set`, `Stack` and `Queue`. It preserves fixed array slots, insertion
generations, strong CAS outcomes, duplicate rules, LIFO/FIFO order and coherent
snapshots. Every operation is applied at one explicit model linearization point.

`is_linearizable` accepts a bounded history with invocation and response steps.
It derives only real-time precedence (`response < invocation`) and exhaustively
searches the remaining orders, comparing every observed outcome with a fresh
sequential state. Histories are capped at twelve operations so a failing case
is reproducible and bounded.

## Cursor, alias and cleanup oracle

The cursor model records only the source kind, finite structural horizon,
position and last map key. It never copies values. Removal before `next` may
omit an entry; replacement is observed at the next linearization point; a
remove/reinsert receives a new generation outside the old horizon. Stack and
queue cursors are observational and cannot pop or dequeue.

`SharedCollectionModel` gives copied handles one identity, retains a source
while a cursor exists, rejects stale tokens before state access and records one
cleanup when the last handle/cursor is gone. The deterministic schedule covers
aliasing, terminal drops, cursor early teardown, limits and stale-token paths.

## Fuzz boundary and promotion

`stdlib_sync_collections` consumes at most 4096 input bytes and 512 transitions.
Each input is replayed twice; invariants are checked after every transition and
all handles and cursors are torn down before returning. The corpus and smoke
runner are `fuzz/corpus/stdlib_sync_collections/seed` and
`scripts/stdlib-sync-collection-fuzz.sh`.

The block is verified for the independent model and regression evidence. The
next leaves are target-qualified collection performance, VM/native conformance,
the broader `std.sync` conformance campaign and documentation. The existing
native ABI remains private and the AOT boundary remains explicitly unclaimed.
