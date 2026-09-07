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
operations. The public worker now emits bounded, sequenced node and cleanup
transitions to a coordinator watchdog. Each active phase has its own deadline;
the coordinator pauses the parent while a descendant executes and applies the
closed setup/teardown caps, reduced by an explicit CLI timeout when supplied.

The 2026-09-07 audit exercised one suite with two leaves, each awaiting
`time.sleep(time.Duration.fromNanoseconds(350000000))`. Each selected alone
passed with `--timeout 500ms`. Selecting both caused exit 3 and
`test worker timed out`, while both passed with `--timeout 1500ms`. This
contradicted the independent body/setup/teardown deadlines in testing section
7.8. The regression now passes with independent setup, nested leaves and
teardown. A deadline request names the active phase generation; the VM cancels
that test boundary, drains observable cleanup and preserves sibling execution.
Timeout is reported separately from language panic or external interruption.

The coordinator allows the bounded cleanup grace before reaping a worker that
does not respond. Failure to establish clean isolation is infrastructure, and
does not produce a successful report. Worker bootstrap and final transport also
have a finite 30-second envelope. Completed leaf results survive a suite
teardown timeout. Retry integration uses the same immutable compiled artifact
and independent process for each selected retry unit.

This does not close all structural limits per phase or prove every native and
non-cooperative cleanup route. `UTEST-LIMIT-001` and T0 remain open until those
remaining integration boundaries and the required gates are verified.
