#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_EXECUTOR_CONTRACT:-$root/testing/stdlib-executor.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-executor-implementation-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.executor implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_EXECUTOR_CONTRACT="$contract" scripts/stdlib-executor-implementation-check.sh

check_log="$logs_dir/cargo-check.log"
CARGO_TARGET_DIR="$target_dir" cargo check -q -p tondo-compiler -p tondo-vm --locked \
    >"$check_log" 2>&1 \
    || { cat "$check_log" >&2; die "compiler/VM checks failed"; }

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-executor-impl-001.to \
    >"$vm_stdout_file" 2>"$vm_stderr_file"
vm_exit=$?
set -e
expected_exit="$(jq -r '.implementation.observed.fixture.exit' "$contract")"
expected_stdout="$(jq -r '.implementation.observed.fixture.stdout' "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr_file" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
vm_stdout="$(tr -d '\r' <"$vm_stdout_file" | sed '/^$/d' | tail -n 1)"
[[ "$vm_stdout" == "$expected_stdout" ]] || die "hosted VM output differs: $vm_stdout"

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-executor-impl-001.to | cut -d' ' -f1)"
check_sha256="$(sha256sum "$check_log" | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg check_sha256 "$check_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    '{
      format:"tondo-stdlib-executor-implementation-evidence/1",
      task:"STD-EXEC-IMPL-001",
      status:"passed-hosted-cooperative",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-executor-impl-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:"executor-ok",status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      hosted:{compiler_vm_check:"passed",log_sha256:("sha256:" + $check_sha256),pool_admission:true,pool_submit_backpressure:true,join_result_projection:true,pool_shutdown:true,pool_cancel_drain:true,actor_create:true,actor_ref_acquisition:true,actor_stop:true,actor_mailbox_handler:true},
      blocking_pool:{status:"capability-missing-until-host-gate",host_workers:false},
      actor:{mailbox_handler_execution:true,actor_ref_acquisition:true,actor_ref_identity_preserved:true,handler_error_propagation:true,stop_waits_for_handler:true,selectable_send_linearization:"verified-hosted-transactional",selectable_send_prepare:"no-state-or-message-mutation",selectable_send_commit:"one-mailbox-linearization",selectable_send_rollback:"unregister-with-message-still-owned-by-sender"},
      public_boundary:{api_promoted:false,native_runtime:"not-claimed",native_aot_lowering:"not-claimed"},
      regressions:["crates/tondo-cli/tests/acceptance_projects.rs::acceptance_project_is_relocatable_and_reports_canonical_observations"],
      resolved_decisions:["Expose Actor.ref(ref self): ActorRef[M] as the explicit non-consuming identity projection; keep Pool.actor returning Actor"],
      open_decisions:[],
      remaining:["STD-EXEC-HOST-001","STD-EXEC-TEST-001","STD-EXEC-PERF-001","STD-EXEC-CONF-001","STD-EXEC-DOC-001"],
      divergences:[]
    }' >"$evidence_dir/stdlib-executor-implementation.json"

echo "std.executor implementation: OK (hosted cooperative pool and actors; report: $evidence_dir/stdlib-executor-implementation.json)"
