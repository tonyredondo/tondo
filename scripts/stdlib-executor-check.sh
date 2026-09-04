#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_EXECUTOR_CONTRACT:-$root/testing/stdlib-executor.json}"

die() {
    echo "std.executor contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.executor"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-EXEC-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-executor.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B4"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .capability.cooperative_pool == "available-without-threads"
  and .capability.blocking_pool == "requires-threads"
  and .capability.missing_threads == "static-capability-error"
  and .capability.ambient_lookup == false
  and .capability.scheduler_blocking == "forbidden"
  and .host.status == "verified-hosted-and-target-qualified-native-bridge"
  and .host.reason == "std.executor has bounded hosted worker admission plus a private x86_64 Linux native token bridge; public API promotion and generic AOT callable lowering remain closed"
  and .surface.types == [
    "Pool",
    "BlockingPool",
    "Actor[S, M, E]",
    "ActorRef[M]",
    "ExecutorError = { InvalidWorkers, InvalidCapacity, ResourceLimit, CapabilityMissing }",
    "SubmitError = { Saturated, Closed, Cancelled, ResourceLimit }",
    "ActorSendError[M] = { Saturated(M), Closed(M), Cancelled(M), Terminated(M), ResourceLimit(M) }"
  ]
  and ([.surface.signatures[].id] | unique) == [
    "actor-ref", "actor-send", "actor-stop", "actor-try-send",
    "blocking-cancel", "blocking-pool", "blocking-run", "blocking-shutdown",
    "pool", "pool-actor", "pool-cancel", "pool-shutdown", "pool-submit", "pool-try-submit"
  ]
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == [
    "actor-stop", "blocking-cancel", "blocking-run", "blocking-shutdown",
    "pool-cancel", "pool-shutdown", "pool-submit"
  ]
  and ([.surface.signatures[] | select(.effect == "selectable") | .id] | sort) == ["actor-send"]
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "required"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == ["actor-send"]
  and .ownership.pool_affine == true
  and .ownership.blocking_pool_affine == true
  and .ownership.pool_copy == false
  and .ownership.blocking_pool_copy == false
  and .ownership.pool_clone == false
  and .ownership.blocking_pool_clone == false
  and .ownership.pool_discard == false
  and .ownership.blocking_pool_discard == false
  and .ownership.actor_affine == true
  and .ownership.actor_copy == false
  and .ownership.actor_ref_copy == true
  and .ownership.actor_ref_send == true
  and .ownership.actor_ref_share == true
  and .ownership.job_transfer == "moves-on-admission-commit"
  and .ownership.job_on_rejection == "caller-retains-job"
  and .ownership.actor_state_bound == "S: Send + Discard"
  and .ownership.actor_message_bound == "M: Send + Discard"
  and .ownership.terminal_consumers == ["pool-shutdown", "pool-cancel", "blocking-shutdown", "blocking-cancel", "actor-stop"]
  and .ownership.scope_exit == ["terminal-consumer", "transfer"]
  and .ownership.implicit_drop == "compile-error"
  and .ownership.post_terminal_use == "compile-error"
  and .ownership.join_obligation == "caller-consumes-existing-Join-contract"
  and .capacity.workers == "positive-finite"
  and .capacity.queue_capacity == "non-negative-finite"
  and .capacity.zero_queue == "running-slots-only"
  and .capacity.negative_workers == "ExecutorError.InvalidWorkers"
  and .capacity.negative_capacity == "ExecutorError.InvalidCapacity"
  and .capacity.unrepresentable_limits == "ExecutorError.ResourceLimit"
  and .capacity.global_default == "forbidden"
  and .capacity.ambient_cpu_default == "forbidden"
  and .capacity.admission_counts == "running-and-queued-jobs"
  and .capacity.queue_order == "FIFO-admission"
  and .capacity.saturation == "explicit-SubmitError.Saturated"
  and .capacity.no_hidden_polling == true
  and .pool.kind == "cooperative-tondo-scheduler-policy"
  and .pool.submit == "waits-for-admission-or-terminal-state"
  and .pool.try_submit == "immediate-only"
  and .pool.accepted_job == "scheduled-through-existing-spawn-and-Join"
  and .pool.new_task_type == false
  and .pool.scheduler == "existing-structured-scheduler"
  and .pool.running_limit == "workers"
  and .pool.queued_limit == "capacity-after-running-slots"
  and .pool.shutdown == "reject-new-drain-accepted-work-and-cleanup"
  and .pool.cancel == "reject-new-request-cooperative-cancel-and-drain"
  and .pool.cancelled_waiter == "Cancelled"
  and .pool.terminal == "consumed-after-drain"
  and .pool.starvation == "forbidden-by-default-under-cooperative-scheduler"
  and .pool.panic == "normal-unwind-after-cleanup"
  and .blocking.capability == "threads"
  and .blocking.job_effect == "non-suspendible"
  and .blocking.job_forbidden == ["suspends", "spawn", "spawn-thread", "select", "local-loan-escape"]
  and .blocking.run == "waits-for-admission-and-host-result-without-blocking-cooperative-worker"
  and .blocking.workers == "bounded-host-threads"
  and .blocking.queue == "finite-explicit-capacity"
  and .blocking.cancel_queued == "do-not-start-and-release"
  and .blocking.cancel_running == "wait-for-safe-host-return"
  and .blocking.force_kill == false
  and .blocking.ambient_state == "forbidden"
  and .blocking.result_boundary == "typed-outcome-or-host-error-through-normal-result"
  and .blocking.worker_blocking == "allowed-only-inside-blocking-pool"
  and .actor.state_owner == "single-actor-affine-owner"
  and .actor.mailbox == "finite-explicit-capacity"
  and .actor.mailbox_order == "FIFO-by-commit"
  and .actor.handler_concurrency == "one-message-at-a-time"
  and .actor.handler_effect == "suspends-allowed"
  and .actor.message_commit == "moves-only-on-linearization"
  and .actor.message_on_failure == "returned-inside-ActorSendError"
  and .actor.send_select_prepare == "no-state-or-message-mutation"
  and .actor.send_select_commit == "one-mailbox-linearization"
  and .actor.send_select_rollback == "unregister-with-message-still-owned-by-sender"
  and .actor.stop == "close-mailbox-cancel-handler-drain-cleanup"
  and .actor.pending_messages == "discarded-under-M-Discard"
  and .actor.handler_error == "actor-terminal-and-propagated-by-stop"
  and .actor.panic == "normal-unwind-after-mailbox-cleanup"
  and .actor.post_terminal_send == "Terminated(message)"
  and .actor.ref_acquisition == "explicit-nonconsuming-identity-projection"
  and .actor.no_restart == true
  and .actor.no_detached_actor == true
  and .lifecycle.pool_states == ["open", "shutting-down", "cancelling", "drained"]
  and .lifecycle.initial == "open"
  and .lifecycle.shutdown_from == ["open"]
  and .lifecycle.cancel_from == ["open", "shutting-down"]
  and .lifecycle.drain_before_success == true
  and .lifecycle.shutdown_rejects_new == true
  and .lifecycle.cancel_rejects_new == true
  and .lifecycle.terminal_consumers_non_idempotent == true
  and .lifecycle.blocking_cancel_never_force_kills == true
  and .lifecycle.actor_states == ["running", "stopping", "terminated"]
  and .lifecycle.actor_stop_from == ["running"]
  and .lifecycle.actor_stop_drains == true
  and .lifecycle.scope_exit_requires_terminal_or_transfer == true
  and .diagnostics.event_namespace == "std.executor"
  and .diagnostics.events == [
    "pool.create", "pool.submit.wait", "pool.submit.accept", "pool.submit.reject",
    "pool.worker.start", "pool.worker.idle", "pool.worker.stop", "pool.shutdown", "pool.cancel",
    "blocking.submit", "blocking.start", "blocking.finish", "actor.create",
    "actor.send.prepare", "actor.send.commit", "actor.send.rollback",
    "actor.message.start", "actor.message.finish", "actor.terminate"
  ]
  and .diagnostics.required_fields == ["run_id", "task_id", "pool_id", "worker_id", "operation_id", "event_sequence", "state"]
  and .diagnostics.payloads == "omitted-by-default"
  and .diagnostics.source_revision == "required"
  and .diagnostics.runtime_hooks_public == false
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 31
  and .implementation.status == "verified-hosted-blocking-and-native-bridge"
  and .implementation.public_api_promoted == false
  and .implementation.host == "verified-hosted-and-target-qualified-native-bridge"
  and .implementation.required_follow_ups == []
  and .implementation.observed.remaining == []
  and .performance.task == "STD-EXEC-PERF-001"
  and .performance.status == "verified-hosted-vm-and-native-token-x86_64-linux"
  and .performance.contract == "testing/stdlib-executor-performance.json"
  and .performance.documentation == "docs/contracts/stdlib-executor-performance.md"
  and .performance.evidence_report == "target/reliability/evidence/stdlib-executor-performance.json"
  and .performance.target_isolation == "hosted-vm-and-native-runtime-are-never-aggregated"
  and .performance.native_aot == "not-claimed"
  and .performance.remaining == []
  and .conformance.remaining == []
  and .documentation == {
    task: "STD-EXEC-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-executor.md",
    fixture: "tests/runtime/m11-std-executor-doc-001.to",
    command: "scripts/stdlib-executor-doc-check.sh",
    expected_stdout: "executor-doc-ok",
    examples: [
      "scoped-join",
      "bounded-backpressure",
      "actor-mailbox",
      "blocking-bridge",
      "cancel-and-drain"
    ],
    sections: [
      "scopes",
      "pools",
      "actors",
      "blocking",
      "cancellation",
      "shutdown",
      "costs",
      "composition-examples"
    ]
  }
  and .promotion.implementation_pending == .implementation.required_follow_ups
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable executor contract"

for path in \
    docs/contracts/stdlib-executor.md \
    docs/contracts/stdlib-executor-performance.md \
    docs/contracts/stdlib-executor-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-executor-test.json \
    testing/stdlib-executor-performance.json \
    testing/stdlib-executor-conformance.json \
    scripts/stdlib-executor-doc-check.sh \
    scripts/stdlib-executor-doc-test.sh \
    tests/runtime/m11-std-executor-doc-001.to \
    tests/runtime/m11-std-executor-doc-001.stdout \
    tests/runtime/m11-std-executor-doc-001.exit \
    tests/runtime/m11-std-executor-doc-001.capabilities; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-EXEC-001' \
    'pub type Pool' \
    'pub type BlockingPool' \
    'pub fn Pool.submit[T, E]' \
    'pub fn Pool.actor[S: Send + Discard' \
    'pub fn Actor.ref(ref self): ActorRef[M]' \
    'pub fn ActorRef.send(ref self, message: M): Unit ! ActorSendError[M] selectable' \
    'pub fn BlockingPool.run[T, E]' \
    'verified-hosted-and-target-qualified-native-bridge' \
    'STD-EXEC-DOC-001' \
    '## Scopes y pools' \
    '## Costes y límites' \
    '## Ejemplos ejecutables de composición' \
    'scoped-join' \
    'bounded-backpressure' \
    'actor-mailbox' \
    'blocking-bridge' \
    'cancel-and-drain' \
    'DIAG-RUNTIME-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-executor.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-executor.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the executor registry"

echo "std.executor contract: OK (bounded cooperative pools; actors; blocking bridge; explicit lifecycle and capabilities)"
