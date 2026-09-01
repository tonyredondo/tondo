#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT:-$root/testing/stdlib-channel-async-iter.json}"

die() {
    echo "std.channel AsyncIterator: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing iteration contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.channel.async-iter"
  and .parent_owner == "std.channel"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CHANNEL-ASYNC-ITER-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-channel-async-iter.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-channel.json"
  and .async_contract == "testing/stdlib-async.json"
  and .layer == "B1"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted"
  and .surface.identity == "std.channel.Receiver[T]"
  and .surface.protocol == "AsyncIterator[T]"
  and .surface.bound == "T: Discard"
  and .surface.next_signature == "fn next(mut self): T? suspends"
  and .surface.for_form == "for item in receiver"
  and .surface.selection == "async-only-when-no-sync-iterator"
  and .surface.one_element_per_next == true
  and .surface.backpressure == "one-next-at-a-time"
  and .surface.materialization == "generic-collect-only"
  and .surface.channel_specific_collect == "forbidden"
  and .surface.for_await == "forbidden"
  and .surface.new_stream_type == false
  and .surface.public_api_promoted == false
  and .semantics.end == "none-after-last-sender-and-buffer-drained"
  and .semantics.early_exit == "close-receiver-and-discard-pending-values"
  and .semantics.discard_requirement == "proved-before-iterator-selection"
  and .semantics.affine_payload == "manual-receive-and-close-required"
  and .semantics.close_after_for == "compiler-owned-terminal-close"
  and .semantics.cancel_pending_next == "unregister-and-discard-on-cleanup"
  and .semantics.generic_collect == "uses-the-same-next-and-cleanup-boundary"
  and .semantics.no_array_intermediate == true
  and .ownership.receiver_affine == true
  and .ownership.payload_bound == "T: Send"
  and .ownership.iterator_bound == "T: Discard"
  and .ownership.next_receiver_mode == "mut-borrow"
  and .ownership.manual_receive_mode == "ref-borrow"
  and .ownership.pending_discard == "only-after-discard-proof"
  and .ownership.early_exit_preserves_affine_values == true
  and .ownership.implicit_receiver_drop == "compile-error"
  and .runtime.hosted_vm.status == "verified"
  and .runtime.hosted_vm.next_host == "std.channel.Receiver.__asyncIteratorNext"
  and .runtime.hosted_vm.adopt_host == "std.channel.Receiver.__asyncIteratorAdopt"
  and .runtime.hosted_vm.scheduler == "reuses-receive-waiter-and-fifo"
  and .runtime.hosted_vm.cleanup_marker == "endpoint-local-discardable-iterator-view"
  and .runtime.hosted_vm.blocking == "cooperative-poll-and-scheduler-park"
  and .runtime.native_runtime_abi.status == "unchanged-parent-channel-abi"
  and .runtime.native_runtime_abi.aot_lowering == "not-claimed"
  and .runtime.native_aot_lowering == "not-claimed"
  and .implementation.status == "verified-hosted-vm"
  and .implementation.public_api_promoted == false
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 6)
  and (.implementation.tests | type == "array" and length == 11)
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.fixture == {path:"tests/runtime/m11-std-channel-async-iter-001.to",stdout:"channel-async-iter-ok",exit:0,status:"passed"}
  and .implementation.negative_fixture == {path:"tests/compile-fail/m11-std-channel-async-iter-discard.to",codes:["E1105"],status:"passed"}
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-channel-async-iter.json"
  and .implementation.required_follow_ups == []
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 11
  and .promotion.implementation_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-CHANNEL-PERF-001"]
  and .promotion.remaining == []
' "$contract" >/dev/null || die "invalid machine-readable AsyncIterator contract"

for path in \
    docs/contracts/stdlib-channel-async-iter.md \
    docs/contracts/stdlib-channel.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-channel.json \
    testing/stdlib-async.json \
    tests/runtime/m11-std-channel-async-iter-001.to \
    tests/runtime/m11-std-channel-async-iter-001.stdout \
    tests/runtime/m11-std-channel-async-iter-001.exit \
    tests/compile-fail/m11-std-channel-async-iter-discard.to \
    tests/compile-fail/m11-std-channel-async-iter-discard.codes; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

while IFS= read -r test; do
    case "$test" in
        scripts/*)
            [[ -x "$root/$test" ]] || die "test script is not executable: $test"
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

for marker in \
    'ChannelReceiverAsyncIteratorNext' \
    'ChannelReceiverAsyncIteratorAdopt' \
    'channel_iterator_receivers' \
    'lower_async_iterator_collect' \
    'channel_host_async_iterator_next_reuses_receive_waiter_and_cleanup' \
    'channel_host_async_iterator_adoption_allows_zero_limit_cleanup' \
    'channel_host_async_iterator_cancellation_allows_terminal_cleanup'; do
    grep -Fq "$marker" \
        "$root/crates/tondo-compiler/src/hir.rs" \
        "$root/crates/tondo-compiler/src/hir/check.rs" \
        "$root/crates/tondo-compiler/src/hir/lower.rs" \
        "$root/crates/tondo-compiler/src/mir/lower.rs" \
        "$root/crates/tondo-compiler/src/process_host.rs" \
        || die "implementation marker is missing: $marker"
done

jq -e '
  .iterator.contract == "testing/stdlib-channel-async-iter.json"
  and (.implementation.required_follow_ups | index("STD-CHANNEL-ASYNC-ITER-001")) == null
  and (.promotion.implementation_pending | index("STD-CHANNEL-ASYNC-ITER-001")) == null
  and .promotion.next_blocks == ["STD-CHANNEL-PERF-001"]
' testing/stdlib-channel.json >/dev/null || die "parent channel registry has a stale AsyncIterator frontier"

jq -e '.iterator.channel_dependency == false' testing/stdlib-async.json >/dev/null \
    || die "std.async registry must remain independent of the channel leaf"

grep -Fq 'testing/stdlib-channel-async-iter.json' TONDO_STANDARD_LIBRARY_SPEC.md \
    || die "stdlib spec does not link the AsyncIterator contract"
grep -Fq 'stdlib-channel-async-iter.md' docs/contracts/stdlib-channel.md \
    || die "channel document does not link the AsyncIterator contract"
grep -Fq 'STD-CHANNEL-ASYNC-ITER-001' TONDO_IMPLEMENTATION_TRACKER.md \
    || die "tracker does not record the AsyncIterator leaf"

[[ "$(tr -d '\r\n' <tests/runtime/m11-std-channel-async-iter-001.exit)" == "0" ]] \
    || die "runtime fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' <tests/runtime/m11-std-channel-async-iter-001.stdout)" == "channel-async-iter-ok" ]] \
    || die "runtime fixture stdout sidecar is not channel-async-iter-ok"
grep -Fxq 'E1105' tests/compile-fail/m11-std-channel-async-iter-discard.codes \
    || die "affine compile-fail sidecar does not pin E1105"

echo "std.channel AsyncIterator: OK (Discard-gated hosted adaptation; FIFO waiter reuse; cleanup boundary)"
