#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_CONTRACT:-$root/testing/stdlib-channel.json}"

die() {
    echo "std.channel contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.channel"
  and .parent_owner == "std.async"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CONC-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-channel.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .layer == "B0"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .host.status == "required-after-native-gate"
  and .host.reason == "Channel needs scheduler wakeups, bounded queue storage and atomic endpoint state on VM and native runtimes; it does not read an ambient host capability"
  and .surface.types == [
    "Sender[T]",
    "Receiver[T]",
    "ChannelError = { InvalidCapacity, ResourceLimit }",
    "SendError[T] = { Closed(T), ResourceLimit(T) }",
    "TrySendError[T] = { Full(T), Closed(T), ResourceLimit(T) }",
    "TryReceive[T] = { Item(T), Empty, Closed }"
  ]
  and ([.surface.signatures[].id] | unique) == [
    "bounded", "receiver-close", "receiver-fork", "receiver-receive",
    "receiver-try-receive", "sender-close", "sender-fork", "sender-send",
    "sender-try-send", "unbounded"
  ]
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "selectable") | .id] | sort) == ["receiver-receive", "sender-send"]
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "required"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == ["sender-send", "receiver-receive"]
  and .ownership.sender_copy == false
  and .ownership.receiver_copy == false
  and .ownership.sender_clone == false
  and .ownership.receiver_clone == false
  and .ownership.sender_send == true
  and .ownership.receiver_send == true
  and .ownership.sender_share == true
  and .ownership.receiver_share == true
  and .ownership.sender_discard == "close-endpoint"
  and .ownership.receiver_discard == false
  and .ownership.fork_is_explicit == true
  and .ownership.fork_shares_identity == true
  and .ownership.send_value_commit == "moves-only-on-linearization"
  and .ownership.send_value_on_failure == "returned-inside-error"
  and .ownership.receive_value_commit == "moves-only-on-linearization"
  and .ownership.receiver_close_returns_pending == "only-last-receiver"
  and .ownership.implicit_drop == "sender-closes; receiver-compile-error"
  and .ownership.use_after_close == "compile-error"
  and .ownership.payload_bound == "T: Send"
  and .ownership.iterator_bound == "T: Discard"
  and .capacity.zero == "rendezvous"
  and .capacity.positive == "finite-buffer-with-backpressure"
  and .capacity.negative == "ChannelError.InvalidCapacity"
  and .capacity.unbounded == "explicit-constructor-only"
  and .capacity.unbounded_limit == "finite-runtime-resource-limit"
  and .capacity.queue_order == "FIFO-by-commit"
  and .capacity.capacity_counts_committed_values_only == true
  and .capacity.no_hidden_polling == true
  and .state_machine.states == ["open", "sender-closed", "receiver-closed", "drained"]
  and .state_machine.initial == "open"
  and .state_machine.sender_close == "decrement-sender-count; last-sender-closes-input"
  and .state_machine.receiver_close == "decrement-receiver-count; last-receiver-closes-input"
  and .state_machine.last_sender_with_buffer == "receive-drains-then-none"
  and .state_machine.last_receiver == "wake-senders-with-intact-payload-and-return-buffer"
  and .state_machine.both_closed == "drained"
  and .state_machine.close_is_idempotent == false
  and .state_machine.post_close_operation == "compile-error"
  and .send.blocking == "waits-for-space-or-receiver"
  and .send.success == "Unit-after-commit"
  and .send.closed == "Closed(value)"
  and .send.resource_limit == "ResourceLimit(value)"
  and .send.try_full == "Full(value)"
  and .send.select_prepare_mutates == false
  and .send.select_commit == "one-linearization-and-value-move"
  and .send.select_rollback == "unregister-with-value-still-owned-by-sender"
  and .send.cancel_before_commit == "value-remains-with-caller"
  and .send.no_duplicate_commit == true
  and .receive.blocking == "waits-for-value-or-closed-and-drained"
  and .receive.success == "Some(value)"
  and .receive.closed_and_drained == "none"
  and .receive.try_empty == "Empty"
  and .receive.try_closed == "Closed"
  and .receive.select_prepare_mutates == false
  and .receive.select_commit == "one-linearization-and-value-move"
  and .receive.select_rollback == "unregister-with-value-still-in-channel"
  and .receive.cancel_before_commit == "no-value-removed"
  and .receive.no_duplicate_receive == true
  and .iterator.protocol == "AsyncIterator[T]"
  and .iterator.bound == "T: Discard"
  and .iterator.one_element_per_next == true
  and .iterator.end == "none-after-last-sender-and-buffer-drained"
  and .iterator.backpressure == "one-next-at-a-time"
  and .iterator.early_exit == "receiver-close-and-discard-pending-values"
  and .iterator.affine_values == "manual-receive-and-close-required"
  and .iterator.materialization == "forbidden"
  and .fairness.waiter_order == "FIFO-registration-per-operation"
  and .fairness.same_channel_tie == "oldest-compatible-registration"
  and .fairness.select_tie == "core-select-rotation"
  and .fairness.starvation_default == "forbidden-under-cooperative-scheduler"
  and .fairness.cross_channel_order == "not-promised"
  and .fairness.scheduler_order_is_promise == false
  and .cancellation.waiting_send == "unregister-before-commit-and-retain-value"
  and .cancellation.waiting_receive == "unregister-before-commit-and-retain-channel-value"
  and .cancellation.close_wakes_waiters == true
  and .cancellation.cleanup_before_scope_exit == true
  and .cancellation.lost_select_branch == "rollback-before-unwind"
  and .cancellation.no_detached_waiters == true
  and .diagnostics.event_namespace == "std.channel"
  and .diagnostics.events == [
    "channel.create", "channel.sender.fork", "channel.receiver.fork",
    "channel.send.prepare", "channel.send.commit", "channel.send.rollback",
    "channel.receive.prepare", "channel.receive.commit", "channel.receive.rollback",
    "channel.sender.close", "channel.receiver.close", "channel.wake", "channel.drain"
  ]
  and .diagnostics.required_fields == ["run_id", "task_id", "channel_id", "endpoint_id", "event_sequence", "state", "capacity", "queued"]
  and .diagnostics.payloads == "omitted-by-default"
  and .diagnostics.source_revision == "required"
  and .diagnostics.runtime_hooks_public == false
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 27
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .promotion.implementation_pending == [
    "STD-CHANNEL-IMPL-001",
    "STD-CHANNEL-ASYNC-ITER-001",
    "STD-CHANNEL-TEST-001",
    "STD-CHANNEL-PERF-001",
    "STD-CHANNEL-CONF-001",
    "STD-CHANNEL-DOC-001"
  ]
  and .promotion.next_blocks == ["STD-ID-001", "STD-LOG-001", "DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable channel contract"

for path in \
    docs/contracts/stdlib-channel.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-CONC-001' \
    'pub type Sender[T]' \
    'pub fn Sender.send(ref self, value: T): Unit ! SendError[T] selectable' \
    'pub fn Receiver.receive(ref self): T? selectable' \
    'moves-only-on-linearization' \
    'FIFO-registration-per-operation' \
    'channel.send.rollback' \
    'required-after-native-gate'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-channel.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-channel.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the channel registry"

echo "std.channel contract: OK (typed endpoints; bounded/unbounded backpressure; cancel-safe select; FIFO fairness)"
