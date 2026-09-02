#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_CONTRACT:-$root/testing/stdlib-channel.json}"

die() {
    echo "std.channel implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.channel"
  and .task == "STD-CONC-001"
  and .status == "contract-locked"
  and .target == "tondo-vm-hosted-and-native"
  and .host.status == "verified-scheduler-and-native-bridge"
  and .host.cooperative_model == "scheduler-owned-poll-and-reacquire"
  and .host.native_bridge == "private-u64-channel-condvar-and-drain"
  and .host.blocking_native_workers_only == true
  and .implementation.status == "verified-hosted-vm-and-native-runtime-abi"
  and .implementation.public_api_promoted == false
  and .implementation.native_status == "verified-native-runtime-abi"
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.algorithmic_fast_paths == "deferred-to-native-targeted-performance-campaign"
  and (.implementation.sources | type == "array" and length == 10)
  and (.implementation.tests | type == "array" and length == 11)
  and .implementation.fixture == {path:"tests/runtime/m11-std-channel-impl-001.to",stdout:"channel-ok",exit:0,status:"passed"}
  and .implementation.native_probe == {path:"crates/tondo-native-runtime/examples/channel_conformance.rs",status:"passed",cases:4,target_policy:"host-target-only-until-native-aot-channel-lowering"}
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-channel-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == []
  and .promotion.implementation_pending == .implementation.required_follow_ups
  and .performance.task == "STD-CHANNEL-PERF-001"
  and .performance.status == "verified-hosted-vm-baseline"
  and .performance.target == "tondo-vm-hosted"
  and .performance.native_aot == "not-claimed"
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-EXEC-IMPL-001"]
' "$contract" >/dev/null || die "invalid machine-readable channel implementation contract"

for path in \
    docs/contracts/stdlib-channel.md \
    docs/contracts/stdlib-channel-async-iter.md \
    docs/contracts/stdlib-channel-test.md \
    docs/contracts/stdlib-channel-performance.md \
    docs/contracts/stdlib-channel-conformance.md \
    docs/contracts/native-abi.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-channel-impl-001.to \
    tests/runtime/m11-std-channel-impl-001.stdout \
    tests/runtime/m11-std-channel-impl-001.exit \
    tests/compile-fail/m11-std-channel-await.to \
    tests/compile-fail/m11-std-channel-await.codes \
    crates/tondo-native-runtime/examples/channel_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing implementation input: $path"
done

for path in \
    testing/stdlib-channel-performance.json \
    scripts/stdlib-channel-performance-check.sh \
    scripts/stdlib-channel-performance-test.sh \
    scripts/stdlib-channel-performance.sh; do
    [[ -f "$root/$path" ]] || die "missing performance boundary input: $path"
done
for path in \
    testing/stdlib-channel-conformance.json \
    scripts/stdlib-channel-conformance-check.sh \
    scripts/stdlib-channel-conformance-test.sh \
    scripts/stdlib-channel-conformance.sh; do
    [[ -f "$root/$path" ]] || die "missing conformance boundary input: $path"
done
for path in \
    docs/contracts/stdlib-channel.md \
    scripts/stdlib-channel-doc-check.sh \
    scripts/stdlib-channel-doc-test.sh \
    tests/runtime/m11-std-channel-doc-001.to \
    tests/runtime/m11-std-channel-doc-001.stdout \
    tests/runtime/m11-std-channel-doc-001.exit; do
    [[ -f "$root/$path" ]] || die "missing documentation boundary input: $path"
done
for script in \
    scripts/stdlib-channel-doc-check.sh \
    scripts/stdlib-channel-doc-test.sh; do
    [[ -x "$root/$script" ]] || die "documentation runner is not executable: $script"
done

for script in \
    scripts/stdlib-channel-implementation-check.sh \
    scripts/stdlib-channel-implementation-test.sh \
    scripts/stdlib-channel-implementation.sh; do
    [[ -x "$root/$script" ]] || die "implementation runner is not executable: $script"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

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
done < <(jq -r '.implementation.tests[]' "$contract")

for symbol in \
    tondo_rt_channel_bounded \
    tondo_rt_channel_unbounded \
    tondo_rt_channel_sender \
    tondo_rt_channel_receiver \
    tondo_rt_channel_sender_fork \
    tondo_rt_channel_receiver_fork \
    tondo_rt_channel_sender_close \
    tondo_rt_channel_receiver_close \
    tondo_rt_channel_send \
    tondo_rt_channel_try_send \
    tondo_rt_channel_receive \
    tondo_rt_channel_try_receive \
    tondo_rt_channel_drain_len \
    tondo_rt_channel_drain_next \
    tondo_rt_channel_waiters; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native channel symbol is missing: $symbol"
done

for marker in \
    'ChannelSenderSend' \
    'ChannelReceiverReceive' \
    'bootstrap_channel_nominals' \
    'pending_channel_for' \
    'poll_channel' \
    'host_call_wakes_select' \
    'RuntimeHostValueKind::ChannelSender' \
    'RuntimeHostValueKind::ChannelReceiver' \
    'channel_host_kind'; do
    grep -Fq "$marker" \
        "$root/crates/tondo-compiler/src/hir.rs" \
        "$root/crates/tondo-compiler/src/hir/check.rs" \
        "$root/crates/tondo-compiler/src/hir/lower.rs" \
        "$root/crates/tondo-compiler/src/resolve.rs" \
        "$root/crates/tondo-compiler/src/process_host.rs" \
        "$root/crates/tondo-vm/src/runtime.rs" \
        "$root/crates/tondo-vm/src/runtime/execute.rs" \
        || die "compiler/host channel anchor is missing: $marker"
done

fixture_root="${root}/tests/runtime/m11-std-channel-impl-001"
[[ "$(tr -d '\r\n' <"$fixture_root.exit")" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' <"$fixture_root.stdout")" == "channel-ok" ]] \
    || die "fixture stdout sidecar is not channel-ok"
grep -Fq 'E1611' "$root/tests/compile-fail/m11-std-channel-await.codes" \
    || die "direct await negative does not assert E1611"

for marker in \
    'STD-CHANNEL-IMPL-001' \
    'verified-scheduler-and-native-bridge' \
    'bridge nativo privado' \
    'ChannelDrain' \
    'hosted VM' \
    'lowering AOT'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-channel.md" \
        || die "implementation document misses marker: $marker"
done

echo "std.channel implementation: OK (hosted scheduler; private native ABI; selectable send/receive; terminal drain)"
