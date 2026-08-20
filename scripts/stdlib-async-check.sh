#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ASYNC_CONTRACT:-testing/stdlib-async.json}"
die() {
    echo "std.async owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.async"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-contract"
  and .contract == "docs/contracts/stdlib-async.md"
  and .layer == "A1"
  and .kind == "intrinsic"
  and .target == "tondo-vm-hosted"
  and .surface.effect == "suspends"
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.inference_by_name == false
  and .surface.effect_in_interface_and_abi == true
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_handle == "required"
  and .surface.nosuspend_annotations == ["@sync", "@nosuspend"]
  and .types == ["Join[T, E]", "Waiter[T, E]", "Completer[T, E]", "AlreadyCompleted", "AsyncIterator[T]"]
  and ([.signatures[].id] | unique) == ["async-iterator-collect", "async-iterator-next", "completer-cancel", "completer-complete", "completer-fail", "oneshot", "waiter-wait"]
  and ([.signatures[] | select(.effect == "suspends") | .id] | sort) == ["async-iterator-collect", "async-iterator-next", "waiter-wait"]
  and all(.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0))
  and .join.origin == ["spawn call()", "spawn thread call()"]
  and .join.affine == true
  and .join.consumption == "await-handle-once"
  and .join.scope_exit == ["await", "cancel-and-await", "detach", "transfer"]
  and .join.public_constructor == false
  and .join.public_polling == false
  and .oneshot.completion == "exactly-one-atomic-winner"
  and .oneshot.duplicate == "AlreadyCompleted"
  and .oneshot.waiter_consumption == "once"
  and .oneshot.completer_send == true
  and .oneshot.callbacks == false
  and .iterator.element_per_next == 1
  and .iterator.end == "none"
  and .iterator.lazy == true
  and .iterator.backpressure == "one-next-at-a-time"
  and .iterator.close == "exactly-once-idempotent-before-terminal-outcome"
  and .iterator.explicit_for_await == "forbidden"
  and .iterator.channel_dependency == false
  and .collect.limit == "finite-non-negative-maximum-elements"
  and .collect.zero == "empty-array-and-close"
  and .collect.at_limit == "success-without-extra-next"
  and .collect.partial_publication == false
  and .collect.error_type == "CollectionError"
  and .collect.close_on == ["success", "error", "cancel", "unwind"]
  and .implementation.status == "verified"
  and .implementation.routes == ["implicit-direct-wait", "spawn-hir-mir-bytecode-vm"]
  and .implementation.cancellation == "structured-scope-cooperative"
  and .implementation.close == "owner-state-released-on-terminal-outcome"
  and .implementation.capacity_failure == "CollectionError-without-partial-publication"
  and ([.test_matrix[].id] | unique | length) == 7
  and .promotion.implementation_pending == []
  and .promotion.next == "STD-A-FUZZ-001"
' "$contract" >/dev/null || die "invalid std.async owner contract"

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || die "missing contract reference: $ref"
done < <(jq -r '.contract, "TONDO_STANDARD_LIBRARY_SPEC.md", "TONDO_IMPLEMENTATION_TRACKER.md"' "$contract")

for marker in \
    'firma sin cuerpo' \
    'inferencia' \
    'Join' \
    'AsyncIterator' \
    'collect(limit:)' \
    'Channel' \
    'backpressure'; do
    grep -Fq "$marker" docs/contracts/stdlib-async.md || die "contract document misses marker: $marker"
done

echo "std.async owner contract: OK (effect-visible surface; Join/oneshot/AsyncIterator/collect bounded)"
