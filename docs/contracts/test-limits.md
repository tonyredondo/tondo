# Finite test limits and phase deadlines

**Status:** implemented for `UTEST-LIMIT-001`

`tondo_compiler::test_limits` defines the coordinator-side resource profile
used by every leaf and suite phase. Defaults are finite for work, memory,
depth, output, artifacts, snapshots, metadata, virtual timers, ready queues
and instructions. The host-independent profile can represent a disabled
wall-clock timeout, but the canonical CLI defaults and any sidecar always
supply a positive cap and reject `--timeout none`.

`LimitProfile::canonical_bytes` and its SHA-256 identify the effective values
without host paths or map-order dependence. The existing sealed envelope gets
its output/artifact/snapshot limits through `envelope_limits`, leaving one
conversion boundary instead of duplicate budget semantics.

## Atomic accounting

`BudgetLedger::reserve` folds duplicate dimensions, preflights every delta and
commits all charges only when every dimension fits. A failed operation leaves
all counters unchanged; zero, overflow and exhausted reservations are typed
errors. The effective profile is available for report publication.

## Timeouts and interruption

`PhaseDeadline` uses monotonic integer nanoseconds. A suite can pause its own
phase while it waits for selected descendants; paused time is excluded from
the setup/teardown deadline. `None` represents an intentionally disabled
wall-clock deadline for non-sidecar consumers and does not disable any
structural budget. `InterruptController` models the first cancellation
request, one finite grace period and forced termination of a non-cooperative
worker. Clock regressions are rejected rather than wrapped.
