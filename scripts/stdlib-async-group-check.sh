#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ASYNC_GROUP_CONTRACT:-$root/testing/stdlib-async-group.json}"

die() {
    echo "std.async.Group contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.async.group"
  and .parent_owner == "std.async"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-ASYNC-GROUP-SPEC-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-async.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .layer == "B1"
  and .kind == "intrinsic"
  and .target == "tondo-vm-hosted"
  and .host.status == "not-applicable"
  and .host.reason == "Group composes the existing scheduler and Join; it has no host bridge or host primitive of its own"
  and .surface.types == [
    "Group[T, E]",
    "Completion[T, E] = { index: Int, outcome: T ! E }"
  ]
  and ([.surface.signatures[].id] | unique) == [
    "group", "group-add", "group-all", "group-cancel", "group-next", "group-settle"
  ]
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == ["group-all", "group-cancel", "group-settle"]
  and ([.surface.signatures[] | select(.effect == "selectable") | .id]) == ["group-next"]
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "required"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == ["group-next"]
  and .ownership.affine == true
  and .ownership.copy == false
  and .ownership.clone == false
  and .ownership.discard == false
  and .ownership.join_add_transfers == "wait-cancel-cleanup-obligation-to-group"
  and .ownership.group_transfer == "allowed-when-send"
  and .ownership.terminal_consumers == ["group-all", "group-settle", "group-cancel"]
  and .ownership.next_preserves_group_owner == true
  and .ownership.scope_exit == ["terminal-consumer", "transfer"]
  and .ownership.implicit_drop == "compile-error"
  and .ownership.post_terminal_use == "compile-error"
  and .ownership.join_use_after_add == "compile-error"
  and .state_machine.states == ["open", "waiting", "ready-to-consume", "consumed"]
  and .state_machine.initial == "open"
  and .state_machine.add_from == ["open", "waiting"]
  and .state_machine.add_to == "open"
  and .state_machine.next_from == ["open", "waiting", "ready-to-consume"]
  and .state_machine.next_to == ["waiting", "ready-to-consume"]
  and .state_machine.terminal_from == ["open", "waiting", "ready-to-consume"]
  and .state_machine.terminal_to == "consumed"
  and .state_machine.none_is_terminal == false
  and .state_machine.terminal_obligation_after_next_none == true
  and .state_machine.completion_queue_is_fifo_by_observation == true
  and .ordering.index_base == 0
  and .ordering.index_assignment == "monotonic-insertion-order"
  and .ordering.index_stability == "stable-for-group-lifetime"
  and .ordering.all_values == "insertion-order"
  and .ordering.settle_outcomes == "insertion-order"
  and .ordering.next == "actual-terminal-completion-order"
  and .ordering.next_tie_break == "lower-insertion-index"
  and .ordering.scheduler_order_is_promise == false
  and .ordering.completion_is_observable_only_after_commit == true
  and .ordering.losing_select_branch_mutates == false
  and .all.waits_for_every_child == true
  and .all.empty == "empty-array-success"
  and .all.success_array_is_partial == false
  and .all.error_requests_cancel_of_unfinished == true
  and .all.error_drains_cleanup == true
  and .all.error_priority == "lowest-insertion-index-among-child-errors"
  and .all.cancelled_children_synthesize_error == false
  and .all.panic == "drain-cleanup-then-propagate"
  and .all.result_order == "insertion-order"
  and .settle.waits_for_every_child == true
  and .settle.empty == "empty-array-success"
  and .settle.error_cancels_siblings == false
  and .settle.outcome_per_child == true
  and .settle.outcome_order == "insertion-order"
  and .settle.success_array_is_partial == false
  and .settle.panic == "drain-cleanup-then-propagate"
  and .next.selectable == true
  and .next.empty == "none-immediately"
  and .next.no_completed_child == "suspends-until-one-terminal"
  and .next.removes_one_completion == true
  and .next.none_after_all_removed == "none-with-group-still-affine"
  and .next.losing_select_branch == "rollback-without-removal"
  and .next.winning_select_branch == "commit-one-removal"
  and .next.completion_payload == "value-or-declared-error"
  and .cancel.requests_all_live_children == true
  and .cancel.request_order == "insertion-order"
  and .cancel.drains_all_children == true
  and .cancel.waits_for_cleanup == true
  and .cancel.returns_outcome == "Unit-success"
  and .cancel.empty == "immediate-success"
  and .cancel.idempotent == false
  and .cancel.terminal == true
  and .diagnostics.event_namespace == "std.async.group"
  and .diagnostics.events == [
    "group.create", "group.add", "group.select.prepare", "group.select.commit",
    "group.select.rollback", "group.child.cancel-request", "group.child.terminal",
    "group.drain", "group.consume"
  ]
  and .diagnostics.required_fields == ["run_id", "task_id", "group_id", "child_index", "event_sequence", "state"]
  and .diagnostics.payloads == "omitted-by-default"
  and .diagnostics.source_revision == "required"
  and .diagnostics.runtime_hooks_public == false
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 20
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-applicable"
  and .promotion.implementation_pending == [
    "STD-ASYNC-GROUP-IMPL-001",
    "STD-ASYNC-GROUP-TEST-001",
    "STD-ASYNC-GROUP-PERF-001",
    "STD-ASYNC-GROUP-CONF-001",
    "STD-ASYNC-GROUP-DOC-001"
  ]
  and .promotion.next_blocks == ["STD-EXEC-001", "STD-NET-001"]
' "$contract" >/dev/null || die "invalid machine-readable Group contract"

for path in \
    docs/contracts/stdlib-async.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-ASYNC-GROUP-SPEC-001' \
    'pub type Group[T, E]' \
    'Group.next(var self): Completion[T, E]? selectable' \
    'menor índice entre' \
    'drena todos los hijos' \
    'HOST = not-applicable'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-async.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-async-group.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the Group registry"

echo "std.async.Group contract: OK (affine fan-in semantics; deterministic all/settle; selectable next; no host bridge)"
