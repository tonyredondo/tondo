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
  .implementation.observed.task == "STD-EXEC-IMPL-001"
  and .implementation.observed.status == "partial-hosted-cooperative-pool"
  and .implementation.observed.hosted_vm == "verified-pool-admission-join-and-lifecycle"
  and .implementation.observed.native_runtime == "not-claimed"
  and .implementation.observed.native_aot_lowering == "not-claimed"
  and .implementation.observed.blocking_pool == "capability-missing-until-host-gate"
  and .implementation.observed.actor == "handle-create-and-stop-only"
  and .implementation.observed.public_api_promoted == false
  and .implementation.observed.fixture == {path:"tests/runtime/m11-std-executor-impl-001.to",stdout:"executor-ok",exit:0,status:"passed"}
  and .implementation.observed.evidence_report == "target/reliability/evidence/stdlib-executor-implementation.json"
  and .implementation.observed.open_decision == "The locked surface has Pool.actor -> Actor but no canonical Actor -> ActorRef acquisition operation"
  and .implementation.observed.remaining == [
    "actor-mailbox-handler-execution",
    "actor-ref-acquisition-contract-decision",
    "STD-EXEC-HOST-001",
    "STD-EXEC-TEST-001",
    "STD-EXEC-PERF-001",
    "STD-EXEC-CONF-001",
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
    'RuntimeHostValueKind::ExecutorPool' \
    'RuntimeActorState' \
    'executor_job_tasks' \
    'join_error_type' \
    'logical_join_value' \
    'is_temporary_join_result'; do
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
    'STD-EXEC-IMPL-001' \
    'partial-hosted-cooperative-pool' \
    'VM hosted' \
    'native AOT' \
    'ActorRef' \
    'STD-EXEC-HOST-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-executor.md" \
        || die "implementation document misses marker: $marker"
done

echo "std.executor implementation: OK (partial hosted cooperative pool; actor/host/AOT boundaries explicit)"
