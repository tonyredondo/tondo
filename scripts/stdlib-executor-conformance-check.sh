#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT:-$root/testing/stdlib-executor-conformance.json}"

die() {
    echo "std.executor conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-executor-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.executor"
  and .task == "STD-EXEC-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-executor.json"
  and .document == "docs/contracts/stdlib-executor-conformance.md"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 9)
  and .vm.expected_stdout[0] == "pool-admission:42:ready"
  and .vm.expected_stdout[1] == "pool-saturation:saturated:drained"
  and .vm.expected_stdout[2] == "blocking-transfer:42:worker"
  and .vm.expected_stdout[3] == "blocking-cancel:cancelled:drained"
  and .vm.expected_stdout[4] == "actor-fifo:3:stopped"
  and .vm.expected_stdout[5] == "actor-terminal:terminated:9"
  and .vm.expected_stdout[6] == "threads-capability:declared"
  and .vm.expected_stdout[7] == "aot-boundary:private-token"
  and .vm.expected_stdout[8] == "executor-conformance-ok"
  and .vm.capabilities_sidecar == "tests/runtime/m11-std-executor-conformance-001.capabilities"
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-executor-lowering"
  and .static_capability.fixture == "tests/compile-fail/m11-std-executor-conf-missing-threads.to"
  and .static_capability.codes == ["E1008"]
  and .static_capability.driver_test == "blocking_pool_requires_an_explicit_threads_target_capability"
  and .static_capability.rule == "blockingPool-requires-explicit-threads-target-capability"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.capability == "threads-is-explicit-static-rejection-without-ambient-lookup"
  and .rules.cleanup == "zero-live-native-handles-before-each-case-result"
  and .rules.native_aot == "not-claimed"
  and (.cases | length == 8)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .native_expected.status == "passed")
  and .cases[0].native_expected == {status:"passed",payload:42,worker:0,lifecycle:"closed",cleanup:true}
  and .cases[1].native_expected == {status:"passed",delegated:"hosted-pool-saturation",native_abi:"blocking-admission-only"}
  and .cases[2].native_expected == {status:"passed",result_tag:2,result_payload:7,managed_transfer:true,cleanup:true}
  and .cases[3].native_expected == {status:"passed",pool_cancelled:true,force_kill:false,lifecycle:"cancelled",cleanup:true}
  and .cases[4].native_expected == {status:"passed",delegated:"hosted-actor-mailbox",native_abi:"blocking-token-only"}
  and .cases[5].native_expected == {status:"passed",delegated:"hosted-actor-lifecycle",native_abi:"blocking-token-only"}
  and .cases[6].native_expected == {status:"passed",target:"x86_64-unknown-linux-gnu",declared:true,static_rejection:"vm-compile-fail"}
  and .cases[7].native_expected == {status:"passed",native_aot:"not-claimed",private_lane:"opaque-token"}
  and (.negative_cases | length == 14)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and .report == "target/reliability/evidence/stdlib-executor-conformance.json"
  and .next_blocks == ["DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-executor.json \
    testing/stdlib-executor-test.json \
    docs/contracts/stdlib-executor.md \
    docs/contracts/stdlib-executor-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-executor-conformance-001.to \
    tests/runtime/m11-std-executor-conformance-001.stdout \
    tests/runtime/m11-std-executor-conformance-001.exit \
    tests/runtime/m11-std-executor-conformance-001.capabilities \
    tests/compile-fail/m11-std-executor-conf-missing-threads.to \
    tests/compile-fail/m11-std-executor-conf-missing-threads.codes \
    crates/tondo-compiler/src/driver.rs \
    crates/tondo-native-runtime/src/lib.rs \
    crates/tondo-native-runtime/examples/executor_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in \
    scripts/stdlib-executor-conformance-check.sh \
    scripts/stdlib-executor-conformance-test.sh \
    scripts/stdlib-executor-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for marker in \
    'executor.pool' \
    'executor.blockingPool' \
    'pool.trySubmit' \
    'actor.ref' \
    'actor.stop' \
    'threads-capability:declared' \
    'aot-boundary:private-token' \
    'executor-conformance-ok'; do
    grep -Fq "$marker" "$root/tests/runtime/m11-std-executor-conformance-001.to" \
        || die "VM corpus marker is missing: $marker"
done

for marker in \
    'HirBootstrapHostFunction' \
    'ExecutorBlockingPool' \
    'capability `threads` is missing for `executor.blockingPool`' \
    'blocking_pool_requires_an_explicit_threads_target_capability'; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/driver.rs" \
        || die "compiler capability marker is missing: $marker"
done

for marker in \
    'tondo_rt_blocking_pool_new' \
    'tondo_rt_blocking_pool_submit' \
    'tondo_rt_blocking_pool_shutdown' \
    'tondo_rt_blocking_pool_cancel' \
    'tondo_rt_blocking_job_wait' \
    'tondo_rt_blocking_job_take' \
    'pool_admission' \
    'pool_saturation_boundary' \
    'managed_transfer' \
    'native_aot'; do
    grep -Fq "$marker" "$root/crates/tondo-native-runtime/src/lib.rs" "$root/crates/tondo-native-runtime/examples/executor_conformance.rs" \
        || die "native conformance marker is missing: $marker"
done

[[ "$(tr -d '\r\n' <tests/runtime/m11-std-executor-conformance-001.exit)" == "0" ]] \
    || die "positive fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' <tests/runtime/m11-std-executor-conformance-001.capabilities)" == "threads" ]] \
    || die "positive fixture capabilities sidecar must declare threads"
mapfile -t expected_lines < <(jq -r '.vm.expected_stdout[]' "$contract")
mapfile -t sidecar_lines < <(tr -d '\r' <tests/runtime/m11-std-executor-conformance-001.stdout | sed '/^$/d')
[[ "${expected_lines[*]}" == "${sidecar_lines[*]}" ]] \
    || die "positive fixture stdout sidecar differs from contract"
grep -Fxq 'E1008' tests/compile-fail/m11-std-executor-conf-missing-threads.codes \
    || die "missing-threads fixture codes sidecar is missing E1008"

for marker in \
    'Eight' \
    'threads' \
    'E1008' \
    'delegated' \
    'opaque tokens' \
    'native AOT' \
    'zero live'; do
    grep -Fqi "$marker" "$root/docs/contracts/stdlib-executor-conformance.md" \
        || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-EXEC-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-executor-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-executor-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.native_aot == "not-claimed"
  and .conformance.cases == 8
  and .conformance.static_capability == "blockingPool-requires-explicit-threads-with-E1008"
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' "$root/testing/stdlib-executor.json" >/dev/null \
    || die "parent executor registry does not expose conformance promotion"

jq -e '.promotion.remaining == []' \
    "$root/testing/stdlib-executor-test.json" >/dev/null \
    || die "executor test registry has a stale conformance frontier"

grep -Fq 'testing/stdlib-executor-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link executor conformance"
grep -Fq 'stdlib-executor-conformance.md' "$root/docs/contracts/stdlib-executor.md" \
    || die "executor document does not link conformance"
grep -Fq 'STD-EXEC-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record executor conformance"

echo "std.executor conformance contract: OK (8 shared cases; threads capability and native boundary)"
