#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_SELECT_CONTRACT:-$root/testing/native-select.json}"

die() {
    echo "native select: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-select/1"
  and .owner == "toolchain.native_runtime"
  and .edition == "0.1"
  and .task == "NATIVE-SELECT-001"
  and .status == "closed"
  and .implementation.runtime == "crates/tondo-native-runtime/src/lib.rs"
  and .implementation.native_adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_select_runs"
  and .machine.phases == ["preparing", "waiting", "committed", "consumed", "else", "rolled-back"]
  and .machine.prepare == "select-begin"
  and .machine.register == ["select-register-task", "select-register-join", "select-register-oneshot", "select-register-time"]
  and .machine.commit == "select-commit"
  and .machine.rollback == "select-rollback"
  and .machine.max_arms == 64
  and .machine.commit_linearization == "one-lock-round-robin-scan"
  and .machine.pending == "returns-not-ready-and-keeps-registration-live"
  and .machine.else == "returns-select-else-and-discards-owned-arms"
  and .machine.duplicate_registration == "rejected-before-commit"
  and .sources.task == "pending-ready-cancelled-joined"
  and .sources.thread == "same-join-state-machine-with-worker-owned-handle"
  and .sources.join == "borrowed-task-owner-consumed-only-by-winning-take"
  and .sources.oneshot == "exactly-one-completion-with-wakeup"
  and .sources.time == "scheduler-fire-transition-with-wakeup"
  and .ownership.winning_arm == "select-take-consumes-the-source-once"
  and .ownership.losing_owned_arm == "ready-source-is-discarded-and-pending-source-is-cancelled"
  and .ownership.losing_join_arm == "remains-owned-by-the-caller"
  and .ownership.rollback == "discards-owned-sources-and-preserves-borrowed-sources"
  and .fairness.policy == "process-local-round-robin"
  and .fairness.rotation == "advances-after-commit-or-else"
  and .fairness.same_ready_set == "successive-commits-start-at-the-next-arm"
  and .wakeup.task == "task-wake-notifies-waiting-selections"
  and .wakeup.oneshot == "oneshot-complete-or-cancel-notifies-waiting-selections"
  and .wakeup.time == "time-fire-notifies-waiting-selections"
  and .wakeup.blocking == false
  and .wakeup.polling_substitute == false
  and .corpus.vm_contract == "testing/async-select-conformance.json"
  and (.corpus.required_cases | length == 3 and unique_values)
  and (.corpus.native_cases | length == 8 and unique_values)
  and .corpus.oracle == "same-VM-selection-observables-and-ownership-rules"
  and (.negative_cases | length == 9 and unique_values)
  and .next_blocks == ["DIAG-NATIVE-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    crates/tondo-native-runtime/src/lib.rs \
    tools/native-evaluation/src/main.rs \
    testing/native-evaluation-runner.json \
    scripts/native-evaluation-runner.sh \
    docs/contracts/native-abi.md \
    docs/contracts/native-evaluation.md; do
    [[ -f "$root/$path" ]] || die "missing native selection input: $path"
done

for symbol in \
    tondo_rt_select_begin \
    tondo_rt_select_register_task \
    tondo_rt_select_register_join \
    tondo_rt_select_register_oneshot \
    tondo_rt_select_register_time \
    tondo_rt_select_commit \
    tondo_rt_select_winner \
    tondo_rt_select_take \
    tondo_rt_select_rollback \
    tondo_rt_select_wakeups \
    tondo_rt_thread_spawn \
    tondo_rt_oneshot_new \
    tondo_rt_oneshot_complete \
    tondo_rt_time_new \
    tondo_rt_time_fire; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "runtime symbol is missing: $symbol"
    grep -Fq "$symbol" "$root/tools/native-evaluation/src/main.rs" \
        || die "native adapter symbol is missing: $symbol"
done

grep -Fq 'native_select_runs' "$root/scripts/native-evaluation-runner.sh" \
    || die "runner does not validate native selection evidence"
grep -Fq 'select-begin' "$root/tools/native-evaluation/src/main.rs" \
    || die "MIR runtime lowering does not include select"
grep -Fq 'round-robin' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "runtime does not document fair rotation"

while IFS= read -r case_id; do
    [[ -n "$case_id" ]] || die "empty corpus case"
    jq -e --arg id "$case_id" '.cases | any(.[]; .id == $id)' \
        "$root/testing/async-select-conformance.json" >/dev/null \
        || die "VM corpus case is missing: $case_id"
done < <(jq -r '.corpus.required_cases[]' "$contract")

while IFS= read -r native_case; do
    [[ "$native_case" =~ ^select-[a-z0-9-]+$ ]] \
        || die "invalid native case: $native_case"
done < <(jq -r '.corpus.native_cases[]' "$contract")

echo "native select contract: OK (atomic prepare/register/commit/rollback, wakeups, fairness and ownership)"
