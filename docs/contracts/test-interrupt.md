# External test interruption transaction

**Status:** implemented for `UTEST-INTERRUPT-001`

`tondo_compiler::test_interrupt` is the coordinator/worker transaction behind
an external test cancellation. The CLI owns OS signal registration and injects
an `InterruptRequest`; this module keeps the signal-independent state machine
deterministic and directly testable.

## Safe path

The first request records its origin and reason, stops dispatching new units,
closes report/store staging, and moves every running worker to `Cancelling`.
Each worker must acknowledge only after user cleanup (including `defer await`)
and revocation of secrets, processes, handles, and resource registrations. A
worker session is then closed. Exit `4` (`Interrupted`) is reachable only when
all registered workers are closed; an empty worker set follows the same safe
path immediately.

The grace clock is the real monotonic clock supplied by the host. Virtual time
cannot advance it. A second request or grace expiry transitions every worker
that is not already closed to `Forced`, records `LostIsolation`, and selects
exit `3`. Late acknowledgements are rejected rather than being mistaken for a
safe cleanup.

## Output transaction

`OutputLedger` models the atomic publication boundary for JSON, JUnit, artifact
manifests, and snapshot updates. Staged values are cleared on interruption;
previous complete outputs remain `Published`, absent paths remain absent, and
staging cannot reopen. The outcome explicitly reports that no machine-readable
output was published. Content-addressed `sha256:<64-hex>` objects may be
recorded as orphan candidates for later garbage collection; arbitrary paths or
digests are rejected. Human output is represented by an unambiguous
`interrupted: ...` line and is not a report oracle.

The state machine exposes injected requests, worker acknowledgements, close
events, dispatch decisions, and monotonic grace polling. It does not register
signals or perform filesystem renames; those effects are wired by
`UTEST-CLI-001` around this transaction.
