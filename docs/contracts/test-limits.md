# Finite test limits and phase deadlines

**Status:** bounded Rust model and partial public enforcement;
`UTEST-LIMIT-001` remains open for per-phase worker integration.

`tondo_compiler::test_limits` models a coordinator-side resource profile
for leaves and suite phases. Defaults are finite for work, memory,
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
worker. Clock regressions are rejected rather than wrapped. These are model
operations; the public worker watchdog currently measures an entire suite
participation with one monotonic start time.

The 2026-09-07 audit exercised one suite with two leaves, each awaiting
`time.sleep(time.Duration.fromNanoseconds(350000000))`. Each selected alone
passed with `--timeout 500ms`. Selecting both caused exit 3 and
`test worker timed out`, while both passed with `--timeout 1500ms`. This
contradicts the independent body/setup/teardown deadlines in testing section
7.8. The public phase transition, watchdog, terminal reporting and retry paths
must be verified together before this task or T0 can close.
