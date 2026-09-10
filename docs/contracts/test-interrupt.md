# External test interruption transaction

**Status:** deterministic coordinator model and executable hosted CLI route;
the reopened testing promotion remains pending.

`tondo_compiler::test_interrupt` is the coordinator/worker transaction behind
an external test cancellation. The CLI owns OS signal registration and injects
an `InterruptRequest`; this module keeps the signal-independent state machine
deterministic and directly testable.

## Safe path

The first request records its origin and reason, stops dispatching new units,
closes report/store staging, and moves every running worker to `Cancelling`.
Each worker must acknowledge only after user cleanup (including `defer`)
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

The executable regression `interrupt_waits_for_inferred_defer_cleanup_before_safe_exit`
keeps a worker in `Cancelling` after an incomplete acknowledgement and reaches
exit 4 only after cleanup (including `defer`) and resource revocation are
both acknowledged. The forced-timeout model is covered separately by
`cleanup_runs_lifo_after_error_and_is_skipped_for_forced_termination`.

## Hosted CLI observation

The CLI registers `ctrlc` 3.5.2 with its `termination` feature. Its signal
thread records the request and notifies its supervisor; ordinary coordinator threads stop dispatch,
join active sessions and apply the coordinator model's real one-second grace
profile. The worker transport declares the exact hashed payload length and
keeps stdin open for a separate cancellation byte. A fixed private stderr
frame notifies the coordinator immediately when the worker itself receives a
signal; this starts its grace clock even if cleanup cannot finish. The final
private interruption response carries cleanup completion, never a full test
report. Each endpoint reports its first and second OS deliveries; the
coordinator merges their counts by maximum so delivery to both coordinator and
worker does not force teardown on the first signal. Supervisor lease closure
requests cleanup without adding another OS delivery. Separate one-off signals
to different endpoints are coalesced into the first request; this transport
has no global OS event identity.

The VM polls external
requests between instructions and during hosted waits, enters the dynamic
unwind drain and closes test envelopes without manufacturing a passed node.
An already running cleanup is shielded from cancellation, including nested
calls and suspension. Second requests and grace expiry terminate and reap the
worker and select exit 3.

Final JSON, JUnit, artifact manifest and mutable snapshot destinations are
captured by a filesystem rollback transaction. An accepted request before the
publication commit restores their previous bytes or absence. Temporary backups
are outside report identity; content-addressed blobs may remain unreferenced.

`external_interrupt_drains_cleanup_and_preserves_complete_outputs` sends real
SIGINT to the public CLI on Unix. It covers a runnable loop, a hosted wait,
cleanup already in progress, grace expiry, second requests to either endpoint,
delivery to both endpoints, structured child teardown and hosted subprocess
cancellation in fourteen cases. It checks
suspendible cleanup, worker exit, stopped subsequent dispatch and preservation
of reports and staged snapshot updates. Both-endpoint cases pause both processes
with SIGSTOP, queue SIGINT at both, then resume them with SIGCONT. This ensures
the second request reaches both endpoints before forced teardown can reap the
worker; single-endpoint cases use ordinary asynchronous delivery. Signal
delivery failures remain test failures and are reported after the coordinator
has been reaped. The subprocess case also verifies
closed stdin and process reaping. This campaign was observed on Linux
x86_64. Windows console events and macOS signals still require target execution
evidence. Declared worker inputs are revoked before result publication and the
coordinator removes its private temporary root after worker reaping. Linux
process-capable targets require the explicit delegated cgroup-v2 provider in
`tondo-cli/src/test_processes.rs`. Before delivering sealed input, the coordinator
moves the worker's whole OS thread group into a fresh child cgroup. The worker
checks its membership against the sealed capability before executing bytecode.
The coordinator owns open kernel control files, uses `cgroup.kill`, waits at most
5,000 ms for `populated 0`, and removes only its own empty group. It does this
before joining inherited pipes, removing temporary files or publishing output.
Retries and repetitions acquire new groups; child cgroup creation is disabled.
The physical delegation path and group names do not enter report identity.

The public process regression covers success, panic and timeout with two
iterations and two jobs. Its explicit shell starts a redirected child in a new
session and exits. Each terminal leaves no live descendant or owned cgroup.
Two additional interruption cases retain such a descendant during cooperative
and forced worker cleanup. The remaining eleven non-process interruption cases
signal readiness through the filesystem; they do not acquire `process` merely
to identify the worker. On unsupported hosts the three process interruption
cases verify rejection before dispatch and output preservation, not signal or
subprocess execution. No unsupported route is replaced by a process group.

This is lifecycle containment within the explicitly delegated OS environment,
not protection against a same-user actor reconfiguring its own delegation.
Kernel cleanup failure stops publication and retains affected temporary paths.
The ordinary `tondo run` process host still has the separate `PROC-008` scope
cleanup gap; this test-worker provider does not close it or promote native AOT.

## Repository validation environment

Linux process lifecycle tests require the explicit `TONDO_TEST_PROCESS_CGROUP`
test-harness input; they fail if it is absent. This variable is never read by
the production CLI. A Linux host with a delegated systemd user manager can run:

```sh
systemd-run --user --scope --quiet --property=Delegate=yes \
  bash scripts/test-process-scope.sh \
  cargo test --workspace --all-targets --locked
```

The script selects the invocation's existing delegated scope and passes its
path to repository test harnesses. It does not change host permissions, install
a service or reuse a preexisting worker group. Native platform probes without
an implemented process provider verify the public rejection boundary. They do
not provide evidence of descendant containment on that host.

Hosted Linux CI uses `scripts/ci-test-scope.sh` because those runners have a
system manager rather than a delegated user manager. It starts a transient
delegated service as the existing runner UID/GID, waits for the check's actual
exit status and collects the unit afterward. The service's control-group kill
policy also closes any remaining test processes when the check exits. Only an
explicit list of build/check environment inputs is forwarded. This does not
install a persistent unit or grant cgroup permissions outside the CI unit.
