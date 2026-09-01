#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-channel-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.channel tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.capacity.zero = "buffered"' testing/stdlib-channel.json > "$tmp_dir/not-rendezvous.json"
expect_failure not-rendezvous env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/not-rendezvous.json" scripts/stdlib-channel-check.sh

jq '.ownership.send_value_on_failure = "discarded"' testing/stdlib-channel.json > "$tmp_dir/lost-payload.json"
expect_failure lost-payload env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/lost-payload.json" scripts/stdlib-channel-check.sh

jq '.send.select_prepare_mutates = true' testing/stdlib-channel.json > "$tmp_dir/send-mutates.json"
expect_failure send-mutates env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/send-mutates.json" scripts/stdlib-channel-check.sh

jq '.fairness.waiter_order = "random"' testing/stdlib-channel.json > "$tmp_dir/random-fairness.json"
expect_failure random-fairness env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/random-fairness.json" scripts/stdlib-channel-check.sh

jq '.iterator.bound = "T: Copy"' testing/stdlib-channel.json > "$tmp_dir/wrong-iterator-bound.json"
expect_failure wrong-iterator-bound env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/wrong-iterator-bound.json" scripts/stdlib-channel-check.sh

jq '.implementation.status = "pending"' testing/stdlib-channel.json > "$tmp_dir/pending-implementation.json"
expect_failure pending-implementation env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/pending-implementation.json" scripts/stdlib-channel-check.sh

jq '.implementation.native_aot_lowering = "verified"' testing/stdlib-channel.json > "$tmp_dir/unclaimed-aot.json"
expect_failure claimed-aot env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/unclaimed-aot.json" scripts/stdlib-channel-check.sh

jq '.host.blocking_native_workers_only = false' testing/stdlib-channel.json > "$tmp_dir/cooperative-blocking.json"
expect_failure cooperative-blocking env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/cooperative-blocking.json" scripts/stdlib-channel-check.sh

jq '.implementation.native_probe.status = "pending"' testing/stdlib-channel.json > "$tmp_dir/pending-probe.json"
expect_failure pending-probe env TONDO_STDLIB_CHANNEL_CONTRACT="$tmp_dir/pending-probe.json" scripts/stdlib-channel-check.sh

for marker in \
    'pub enum ChannelError' \
    'pub enum SendError[T]' \
    'pub enum TrySendError[T]' \
    'pub enum TryReceive[T]' \
    'pub fn bounded[T: Send](capacity: Int): (Sender[T], Receiver[T]) ! ChannelError' \
    'pub fn unbounded[T: Send](): (Sender[T], Receiver[T]) ! ChannelError' \
    'pub fn Sender.send(ref self, value: T): Unit ! SendError[T] selectable' \
    'pub fn Sender.close(self): Unit' \
    'pub fn Receiver.receive(ref self): T? selectable' \
    'pub fn Receiver.close(self): Array[T]'; do
    grep -Fq "$marker" docs/contracts/stdlib-channel.md
done

for marker in \
    'rendezvous' \
    'moves-only-on-linearization' \
    'receiver-close-and-discard-pending-values' \
    'FIFO-registration-per-operation' \
    'channel.receive.rollback' \
    'verified-scheduler-and-native-bridge' \
    'STD-CHANNEL-IMPL-001' \
    'AsyncChannel' \
    'stdlib-select-api'; do
    grep -Fq "$marker" testing/stdlib-channel.json
done

jq -e '
  .task == "STD-CONC-001"
  and .surface.selectable_operations == ["sender-send", "receiver-receive"]
  and .capacity.zero == "rendezvous"
  and .ownership.send_value_on_failure == "returned-inside-error"
  and .send.select_rollback == "unregister-with-value-still-owned-by-sender"
  and .receive.select_rollback == "unregister-with-value-still-in-channel"
  and .iterator.bound == "T: Discard"
  and .fairness.waiter_order == "FIFO-registration-per-operation"
  and .implementation.public_api_promoted == false
  and .implementation.status == "verified-hosted-vm-and-native-runtime-abi"
  and .implementation.native_aot_lowering == "not-claimed"
  and .host.blocking_native_workers_only == true
  and .promotion.next_blocks == ["STD-CHANNEL-TEST-001", "STD-CHANNEL-PERF-001"]
' testing/stdlib-channel.json >/dev/null

echo "std.channel tests: OK (negative contract cases, ownership, backpressure, select, runtime boundary and fairness anchors)"
