#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT:-$root/testing/stdlib-channel-conformance.json}"

die() {
    echo "std.channel conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-channel-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.channel"
  and .task == "STD-CHANNEL-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-channel.json"
  and .document == "docs/contracts/stdlib-channel-conformance.md"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 8)
  and .vm.expected_stdout[0] == "bounded-fifo:12:full:closed"
  and .vm.expected_stdout[1] == "rendezvous-wakeup:7:cleanup"
  and .vm.expected_stdout[2] == "receiver-drain:45:fifo"
  and .vm.expected_stdout[3] == "closed-error:9:payload-preserved"
  and .vm.expected_stdout[4] == "invalid-capacity:rejected"
  and .vm.expected_stdout[5] == "select-commit:1:6"
  and .vm.expected_stdout[6] == "closed-wakeup:42:cleanup"
  and .vm.expected_stdout[7] == "channel-conformance-ok"
  and .vm.panic_expected_exit == 101
  and .vm.panic_expected_stdout == ["channel-panic-cleanup"]
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-channel-lowering"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.fifo == "commit-order-for-buffered-and-rendezvous-values"
  and .rules.errors == "closed-full-invalid-capacity-and-resource-limit-preserve-payload-or-status"
  and .rules.panic == "deferred-close-before-propagation-and-native-unwind-guard"
  and .rules.cleanup == "zero-live-endpoints-and-no-waiters-before-each-case-result"
  and .rules.native_aot == "not-claimed"
  and (.cases | length == 8)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .native_expected.status == "passed")
  and .cases[0].native_expected == {status:"passed",order:[1,2],full_payload:3,closed:true,cleanup:true}
  and .cases[1].native_expected == {status:"passed",value:7,wakeups:true,cleanup:true}
  and .cases[2].native_expected == {status:"passed",pending:[4,5],fifo:true,cleanup:true}
  and .cases[3].native_expected == {status:"passed",payload:9,status_code:13,cleanup:true}
  and .cases[4].native_expected == {status:"passed",negative:true,resource_limit:true,cleanup:true}
  and .cases[5].native_expected == {status:"passed",delegated:"hosted-select-implementation-leaf",native_abi:"private-channel-only"}
  and .cases[6].native_expected == {status:"passed",payload:42,wakeups:true,cleanup:true}
  and .cases[7].native_expected == {status:"passed",panic:true,cleanup:"exactly-once"}
  and (.negative_cases | length == 15)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and .report == "target/reliability/evidence/stdlib-channel-conformance.json"
  and .next_blocks == ["STD-CHANNEL-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-channel.json \
    docs/contracts/stdlib-channel.md \
    docs/contracts/stdlib-channel-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-channel-conformance-001.to \
    tests/runtime/m11-std-channel-conformance-001.stdout \
    tests/runtime/m11-std-channel-conformance-001.exit \
    tests/runtime/m11-std-channel-conformance-panic-001.to \
    tests/runtime/m11-std-channel-conformance-panic-001.codes \
    tests/runtime/m11-std-channel-conformance-panic-001.stdout \
    tests/runtime/m11-std-channel-conformance-panic-001.exit \
    crates/tondo-native-runtime/src/lib.rs \
    crates/tondo-native-runtime/examples/channel_shared_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in \
    scripts/stdlib-channel-conformance-check.sh \
    scripts/stdlib-channel-conformance-test.sh \
    scripts/stdlib-channel-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for symbol in \
    tondo_rt_reset \
    tondo_rt_channel_bounded \
    tondo_rt_channel_sender \
    tondo_rt_channel_receiver \
    tondo_rt_channel_sender_close \
    tondo_rt_channel_receiver_close \
    tondo_rt_channel_send \
    tondo_rt_channel_try_send \
    tondo_rt_channel_receive \
    tondo_rt_channel_try_receive \
    tondo_rt_channel_drain_len \
    tondo_rt_channel_drain_next \
    tondo_rt_channel_waiters \
    tondo_rt_live_objects; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native conformance symbol is missing: $symbol"
done

for marker in \
    bounded_fifo_case \
    rendezvous_case \
    receiver_drain_case \
    closed_error_case \
    invalid_capacity_case \
    closed_wakeup_case; do
    grep -Fq "$marker" "$root/tests/runtime/m11-std-channel-conformance-001.to" \
        || die "VM corpus marker is missing: $marker"
done
grep -Fq 'defer cleanup_receiver(receiver)' \
    "$root/tests/runtime/m11-std-channel-conformance-panic-001.to" \
    || die "panic fixture misses deferred receiver cleanup"
grep -Fq 'panic("channel conformance panic")' \
    "$root/tests/runtime/m11-std-channel-conformance-panic-001.to" \
    || die "panic fixture misses panic marker"

for marker in \
    'eight-case observable corpus' \
    'same case IDs' \
    'ChannelError.InvalidCapacity' \
    'Full(value)' \
    'deferred cleanup' \
    'native AOT' \
    'private native channel ABI' \
    'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-channel-conformance.md" \
        || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-CHANNEL-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-channel-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-channel-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.native_aot == "not-claimed"
  and .conformance.cases == 8
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-EXEC-IMPL-001"]
' "$root/testing/stdlib-channel.json" >/dev/null \
    || die "parent channel registry does not expose conformance promotion"

jq -e '
  .promotion.next_blocks == ["STD-CHANNEL-DOC-001"]
  and .promotion.implementation_pending == []
' "$root/testing/stdlib-channel-test.json" >/dev/null \
    || die "channel test registry has a stale conformance frontier"

jq -e '.promotion.next_blocks == ["STD-CHANNEL-DOC-001"]' \
    "$root/testing/stdlib-channel-async-iter.json" >/dev/null \
    || die "channel AsyncIterator registry has a stale conformance frontier"

grep -Fq 'testing/stdlib-channel-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link channel conformance"
grep -Fq 'stdlib-channel-conformance.md' "$root/docs/contracts/stdlib-channel.md" \
    || die "channel document does not link channel conformance"
grep -Fq 'STD-CHANNEL-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record channel conformance"

[[ "$(tr -d '\r\n' <tests/runtime/m11-std-channel-conformance-001.exit)" == "0" ]] \
    || die "positive fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' <tests/runtime/m11-std-channel-conformance-panic-001.exit)" == "101" ]] \
    || die "panic fixture exit sidecar is not 101"
grep -Fxq 'P0008' tests/runtime/m11-std-channel-conformance-panic-001.codes \
    || die "panic fixture codes sidecar is missing P0008"
grep -Fxq 'channel-panic-cleanup' tests/runtime/m11-std-channel-conformance-panic-001.stdout \
    || die "panic fixture stdout sidecar is missing cleanup marker"

echo "std.channel conformance contract: OK (8 shared cases; errors, panic cleanup and native boundary)"
