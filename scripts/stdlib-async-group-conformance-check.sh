#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="$root/testing/stdlib-async-group-conformance.json"
override="$(printenv TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT || true)"
if [[ -n "$override" ]]; then
    contract="$override"
fi

die() {
    echo "std.async.Group conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-async-group-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.async.group"
  and .task == "STD-ASYNC-GROUP-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-async-group.json"
  and .document == "docs/contracts/stdlib-async-group-conformance.md"
  and .vm.expected_exit == 0
  and .vm.expected_stdout == "group-ok"
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-async-lowering"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.ordering == "zero-based-insertion-and-completion-order"
  and .rules.errors == "lowest-insertion-index-error-after-drain"
  and .rules.panic == "drain-cleanup-then-propagate"
  and .rules.cancellation == "drain-all-live-children-before-return"
  and .rules.cleanup == "exactly-once-and-no-live-child-after-terminal-consumer"
  and (.cases | length == 8)
  and ([.cases[].id] | length == (unique | length))
  and all(.cases[]; (.id | test("^group-[a-z0-9-]+$")) and (.native_expected.status == "passed"))
  and .cases[0].native_expected == {status:"passed",all:"ok",remaining:0,outcomes:2,values:[1,2]}
  and .cases[1].native_expected == {status:"passed",settle:"ok",outcomes:2,values:[7,8],errors:[false,true]}
  and .cases[2].native_expected == {status:"passed",error_tag:3,error_payload:12,pending_drained:true,outcomes:1}
  and .cases[3].native_expected == {status:"passed",indices:[1,0],values:[22,11],none:true}
  and .cases[4].native_expected == {status:"passed",panic:true,cleanup:"exactly-once"}
  and .cases[5].native_expected == {status:"passed",cleanup:"exactly-once"}
  and .cases[6].native_expected == {status:"passed",all:true,settle:true,next_none:true}
  and .cases[7].native_expected == {status:"passed",invalid_handle:true,joined_rejected:true}
  and (.negative_cases | length == 10)
  and (.next_blocks == ["STD-SYNC-IMPL-001"])
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-async-group.json \
    docs/contracts/stdlib-async-group-conformance.md \
    tests/runtime/m11-std-async-group-001.to \
    crates/tondo-native-runtime/src/lib.rs \
    crates/tondo-native-runtime/examples/async_group_conformance.rs \
    scripts/stdlib-async-group-conformance.sh \
    scripts/stdlib-async-group-conformance-test.sh; do
    [[ -e "$root/$path" ]] || die "missing conformance input: $path"
done
[[ -x "$root/scripts/stdlib-async-group-conformance.sh" ]] || die "runner is not executable"
[[ -x "$root/scripts/stdlib-async-group-conformance-test.sh" ]] || die "contract test is not executable"

for symbol in \
    tondo_rt_group_new \
    tondo_rt_group_add \
    tondo_rt_group_next \
    tondo_rt_group_all \
    tondo_rt_group_settle \
    tondo_rt_group_cancel \
    tondo_rt_task_panic \
    tondo_rt_group_outcome_count \
    tondo_rt_group_outcome_index \
    tondo_rt_group_outcome_value \
    tondo_rt_group_outcome_is_error; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native runtime symbol is missing: $symbol"
done

for marker in \
    'same eight logical cases' \
    'lowest insertion-index error' \
    'fresh process' \
    'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-async-group-conformance.md" \
        || die "conformance document misses marker: $marker"
done

echo "std.async.Group conformance contract: OK (8 shared cases; native runtime ABI; fail-closed negatives)"
