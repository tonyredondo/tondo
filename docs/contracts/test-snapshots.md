# Canonical textual snapshots

`tondo-compiler::test_snapshots` implements the snapshot store
`tondo-snapshot-store-0.1/1`.  A key is the exact pair `(node_id, name)` and
the stored value is an exact Tondo `String`; no trimming, Unicode
normalization, newline conversion, or host encoding is performed.  Entries
are sorted by UTF-8 bytes of `node_id` and then `name`, and canonical JSON
bytes provide the store content hash.

Checks are read-only and return `matched`, `missing`, or `mismatched`.  A
mismatch includes SHA-256 values and a bounded human diff; it is classified as
`P2007`.  Duplicate/invalid keys, non-canonical stores, unsafe paths, and
invalid update policy are conflicts under `P2008` where the protocol requires
it.  Stale entries are never removed by a check or update.

`SnapshotUpdateStage` is created for one invocation/attempt with an immutable
original store.  It records explicit `created` or `updated` values, rejects a
second update for the same key, and cannot materialize a store until the
coordinator calls `mark_success`.  The merged store preserves every stale
entry and publishes with write/fsync/rename atomic replacement only after the
whole invocation succeeded.  No implicit update or deletion is possible, and
another attempt owns a separate stage.

Update requires one job, canonical order, no shard, retry, repeat, or
`allow-flaky`.  `load`/`publish_atomic` reject absolute paths, `..` escapes,
symlinked package roots, and symlinked files.  Relative package paths keep
stores for unselected packages intact and keep physical host paths out of the
logical store bytes.

