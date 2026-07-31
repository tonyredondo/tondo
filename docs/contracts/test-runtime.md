# Isolated test leaf runtime

**Status:** implemented for `UTEST-RUNTIME-001`

`tondo_compiler::test_runtime` is the coordinator-facing worker boundary. A
`LeafProgram` is reusable for retries, but each invocation creates a new
bootstrap, worker identity, envelope, resource ledger and empty environment.
Results are detached before revocation and sorted by static leaf ID rather than
completion order.

## Bootstrap and resources

The worker protocol has exactly three phases: `Fresh`, `Initialized` and
`Revoked`. Initialization creates distinct worker, heap and executor IDs and a
single clock-provider boundary. `revoke` is idempotent internally and removes
every tracked resource from the global registry, even if a stale Rust handle is
still alive. A later allocation through that stale worker fails; no root,
handle, buffer or task can cross into another leaf.

The runner schedules at most `jobs` workers at once. Every worker starts with
an empty environment and an independent `EnvelopeHandle`; logs, tags, streams,
artifacts, snapshots, virtual-time observations and budgets consequently stay
per attempt.

## Terminal and cleanup behavior

Body returns, errors, skips, pánicos, resource limits, timeouts and
infrastructure are projected to one closed `RuntimeStatus` vocabulary. User
pánicos are caught so siblings continue. Registered cleanup callbacks run in
LIFO order after ordinary errors and skips; a forced termination records a
timeout and explicitly reports that cleanup was not executed, never pretending
that a `defer` ran. Cleanup failures replace a prior skip and retain the
failure status.

`WorkerContext` only forwards sealed testing operations, opaque resource
allocation and structured-child inheritance. Virtual time uses the same
operation boundary as monotonic time and is revoked with its envelope. A
program can be executed again for a retry, but the worker identity and all
runtime state are different.
