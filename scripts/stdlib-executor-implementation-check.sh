#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_EXECUTOR_CONTRACT:-$root/testing/stdlib-executor.json}"

die() {
    echo "std.executor implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
TONDO_STDLIB_EXECUTOR_CONTRACT="$contract" scripts/stdlib-executor-check.sh >/dev/null

jq -e '
  .implementation.observed.task == "STD-EXEC-HOST-001"
  and .implementation.observed.status == "verified-hosted-blocking-and-native-token-bridge"
  and .implementation.observed.hosted_vm == "verified-blocking-admission-isolated-child-engine-host-adapter-and-lifecycle"
  and .implementation.observed.native_runtime == "verified-target-qualified-x86_64-linux-token-bridge"
  and .implementation.observed.native_aot_lowering == "not-claimed"
  and .implementation.observed.blocking_pool == "verified-hosted-isolated-workers-and-native-token-bridge"
  and .implementation.observed.actor == "mailbox-handler-state-error-stop-and-selectable-send"
  and .implementation.observed.selectable_actor_send == "verified-hosted-transactional"
  and .implementation.observed.public_api_promoted == false
  and .implementation.observed.fixture == {path:"tests/runtime/m11-std-executor-impl-001.to",stdout:"executor-ok",exit:0,status:"passed"}
  and .implementation.observed.evidence_report == "target/reliability/evidence/stdlib-executor-implementation.json"
  and .implementation.observed.resolved_decision == "Expose Actor.ref(ref self): ActorRef[M] as the explicit non-consuming identity projection; keep Pool.actor returning Actor"
  and (.implementation.observed.open_decision // null) == null
  and .implementation.observed.remaining == [
    "STD-EXEC-DOC-001"
  ]
' "$contract" >/dev/null || die "invalid observed executor implementation contract"

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.observed.sources[]' "$contract")

while IFS= read -r test; do
    case "$test" in
        scripts/*)
            [[ -x "$root/$test" ]] || die "implementation test script is not executable: $test"
            ;;
        *::*)
            file="${test%%::*}"
            name="${test##*::}"
            [[ -f "$root/$file" ]] || die "missing test source: $file"
            grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
            ;;
        *)
            [[ -f "$root/$test" ]] || die "missing test source: $test"
            ;;
    esac
done < <(jq -r '.implementation.observed.tests[]' "$contract")

for script in \
    scripts/stdlib-executor-implementation-check.sh \
    scripts/stdlib-executor-implementation-test.sh \
    scripts/stdlib-executor-implementation.sh; do
    [[ -x "$root/$script" ]] || die "implementation runner is not executable: $script"
done

fixture_path="$(jq -r '.implementation.observed.fixture.path' "$contract")"
fixture_root="${root}/${fixture_path%.to}"
[[ -f "$root/$fixture_path" ]] || die "missing implementation fixture: $fixture_path"
[[ -f "${fixture_root}.stdout" ]] || die "missing fixture stdout sidecar: ${fixture_root}.stdout"
[[ -f "${fixture_root}.exit" ]] || die "missing fixture exit sidecar: ${fixture_root}.exit"
[[ "$(tr -d '\r\n' <"${fixture_root}.exit")" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r' <"${fixture_root}.stdout" | sed '/^$/d' | tail -n 1)" == "executor-ok" ]] \
    || die "fixture stdout sidecar is not executor-ok"

for marker in \
    'ExecutorPoolSubmit' \
    'PoolJob' \
    'admit_executor_submit' \
    'begin_executor_pool_lifecycle' \
    'finish_executor_pool_lifecycle' \
    'executor_pool_capacity_available' \
    'executor_pool_constructor' \
    'executor_actor_error_result' \
    'ExecutorActorRef' \
    'RuntimeHostValueKind::ExecutorPool' \
    'RuntimeActorState' \
    'executor_job_tasks' \
    'join_error_type' \
    'logical_join_value' \
    'is_temporary_join_result' \
    'RuntimeSelectReservation' \
    'ActorSelectSend' \
    'actor_select_send_ready' \
    'select_actor_send_move_places' \
    'selectable_actor_send_commit_consumes_message_once' \
    'selectable_actor_send_rollback_unregisters_waiter_and_retains_message' \
    'selectable_actor_send_waiter_wakes_only_after_capacity_opens'; do
    grep -Fq "$marker" \
        "$root/crates/tondo-compiler/src/hir.rs" \
        "$root/crates/tondo-compiler/src/hir/check.rs" \
        "$root/crates/tondo-compiler/src/hir/lower.rs" \
        "$root/crates/tondo-compiler/src/resolve.rs" \
        "$root/crates/tondo-vm/src/bytecode/verify.rs" \
        "$root/crates/tondo-vm/src/runtime.rs" \
        "$root/crates/tondo-vm/src/runtime/execute.rs" \
        || die "compiler/VM executor anchor is missing: $marker"
done

for marker in \
    'NativeBlockingPool' \
    'native_blocking_worker_loop' \
    'tondo_rt_blocking_pool_new' \
    'tondo_rt_blocking_pool_submit' \
    'tondo_rt_blocking_job_wait' \
    'native_blocking_pool_runs_bounded_jobs_and_transfers_payload_once' \
    'native_blocking_queue_cancellation_is_atomic_before_worker_admission'; do
    grep -Fq "$marker" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native executor anchor is missing: $marker"
done

grep -Fq 'executor_actor_ref_projects_live_identity_and_rejects_invalid_handles' \
    "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "Actor.ref runtime regression test is missing"

grep -Fq 'executor_actor_rejects_malformed_handler_before_message_commit' \
    "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "actor handler validation regression test is missing"

grep -Fq 'executor_actor_waits_resume_and_stop_paths_are_explicit' \
    "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "actor wait and stop scheduler regression test is missing"

for marker in \
    'BlockingExecutionBridge' \
    'blocking_worker_loop' \
    'BlockingWorkerHost' \
    'resume_blocking_submit' \
    'resume_blocking_call' \
    'blocking_bridge_runs_verified_job_on_a_host_worker'; do
    grep -Fq "$marker" "$root/crates/tondo-vm/src/runtime/execute.rs" \
        || die "hosted blocking executor anchor is missing: $marker"
done

for marker in \
    'STD-EXEC-IMPL-001' \
    'STD-EXEC-HOST-001' \
    'verified-hosted-blocking-and-native-token-bridge' \
    'VM hosted' \
    'runtime nativo' \
    'native AOT' \
    'ActorRef' \
    'BlockingPool' \
    'x86_64-unknown-linux-gnu'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-executor.md" \
        || die "implementation document misses marker: $marker"
done

echo "std.executor implementation: OK (hosted blocking bridge and target-qualified native token lane; AOT callable boundary explicit)"
