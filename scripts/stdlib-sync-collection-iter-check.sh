#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_COLLECTION_ITER_CONTRACT:-$root/testing/stdlib-sync-collection-iter.json}"

die() {
    echo "std.sync collection iteration: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing iteration contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.sync.collection.iter"
  and .parent_owner == "std.sync.collection"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-SYNC-COLLECTION-ITER-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-sync-collection-iter.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-sync-collection.json"
  and .sync_contract == "testing/stdlib-sync.json"
  and .layer == "B2"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native-runtime-abi"
  and (.surface.identities | length) == 5
  and .surface.protocol == "AsyncIterator"
  and .surface.cursor_type == "cursor[sync,C]"
  and .surface.binding == "value-only"
  and .surface.source_evaluation == "once"
  and .surface.materialization == "forbidden"
  and .surface.new_keywords == false
  and .surface.public_api_promoted == false
  and .semantics.horizon == "finite-structural-O1"
  and .semantics.horizon_capture == "no-content-copy"
  and .semantics.post_cursor_insertions == "excluded"
  and .semantics.post_cursor_reinsertions == "excluded"
  and .semantics.pre_next_removals == "may-be-omitted"
  and .semantics.replacement == "value-at-next-linearization"
  and .semantics.generation == "at-most-once-even-under-ABA"
  and .semantics.order.array == "ascending-index"
  and .semantics.order.map == "linearized-insertion"
  and .semantics.order.set == "linearized-insertion"
  and .semantics.order.stack == "top-down-LIFO"
  and .semantics.order.queue == "front-first-FIFO"
  and .semantics.stack_queue_consumption == "non-destructive"
  and .semantics.lock_held_in_body == false
  and .semantics.next == "linearizable-and-may-suspend"
  and .semantics.termination == "finite-even-with-continuing-writers"
  and .semantics.snapshot_difference == "snapshot-is-one-linearization-coherent-materialization"
  and .diagnostics.loan_binding == "E1402"
  and .diagnostics.no_suspend_context == "E1601"
  and .diagnostics.stack_queue_missing_capability == "E1105"
  and .ownership.source_handle == "copied-identity"
  and .ownership.cursor_owner == "owning-private-state"
  and .ownership.cursor_copy == false
  and .ownership.cursor_source_lifetime == "strong-edge-until-cursor-release"
  and .ownership.loaned_bindings == "forbidden"
  and .ownership.stack_queue_bounds == "Copy + Send + Share"
  and .runtime.hosted_vm.status == "verified"
  and .runtime.hosted_vm.cursor == "IteratorAdapter::Sync"
  and .runtime.native_runtime_abi.status == "verified-private-abi-cursor"
  and .runtime.native_runtime_abi.carrier == "opaque-u64-capability"
  and .runtime.native_runtime_abi.symbols == [
    "tondo_rt_sync_cursor_start",
    "tondo_rt_sync_cursor_next",
    "tondo_rt_sync_cursor_key"
  ]
  and .runtime.native_runtime_abi.global_lock_while_waiting == false
  and .runtime.native_aot_lowering == "not-claimed"
  and .implementation.status == "verified-hosted-vm-and-private-native-runtime-abi"
  and .implementation.public_api_promoted == false
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 9)
  and (.implementation.tests | type == "array" and length == 6)
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == [
    "STD-SYNC-COLLECTION-TEST-001",
    "STD-SYNC-COLLECTION-PERF-001",
    "STD-SYNC-COLLECTION-CONF-001",
    "STD-SYNC-CONF-001",
    "STD-SYNC-DOC-001"
  ]
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 14
  and .promotion.implementation_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-TEST-001"]
  and .promotion.remaining == .implementation.required_follow_ups
' "$contract" >/dev/null || die "invalid machine-readable iteration contract"

for path in \
    docs/contracts/stdlib-sync-collection-iter.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json \
    testing/stdlib-sync-collection.json \
    docs/contracts/stdlib-sync-collection.md \
    docs/contracts/native-abi.md; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

while IFS= read -r test; do
    if [[ "$test" == scripts/* ]]; then
        [[ -x "$root/$test" ]] || die "iteration test script is not executable: $test"
        continue
    fi
    file="${test%%::*}"
    name="${test##*::}"
    [[ -f "$root/$file" ]] || die "missing test source: $file"
    grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
done < <(jq -r '.implementation.tests[]' "$contract")

for symbol in \
    tondo_rt_sync_cursor_start \
    tondo_rt_sync_cursor_next \
    tondo_rt_sync_cursor_key; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native cursor symbol is missing: $symbol"
done

for marker in \
    'IteratorAdapter::Sync' \
    'sync_cursor_host_descriptor' \
    'sync_collection_direct_for_uses_finite_host_cursor_order' \
    'sync_collection_cursor_preserves_order_horizon_and_reinsertion_boundary' \
    'native_sync_cursor_is_finite_ordered_and_generation_safe' \
    'E1402' \
    'E1601' \
    'E1105'; do
    grep -Fq "$marker" \
        "$root/crates/tondo-compiler/src/hir/check.rs" \
        "$root/crates/tondo-compiler/src/driver.rs" \
        "$root/crates/tondo-compiler/src/process_host.rs" \
        "$root/crates/tondo-vm/src/runtime/heap.rs" \
        "$root/crates/tondo-vm/src/runtime/execute.rs" \
        "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "implementation marker is missing: $marker"
done

jq -e '
  .promotion.next_blocks == ["STD-SYNC-COLLECTION-TEST-001"]
  and (.promotion.implementation_pending | index("STD-SYNC-COLLECTION-ITER-001")) == null
  and (.promotion.implementation_pending | index("STD-SYNC-COLLECTION-TEST-001")) != null
  and .collections.direct_for.protocol == "AsyncIterator"
' "$root/testing/stdlib-sync.json" >/dev/null \
    || die "parent std.sync registry does not expose the iteration promotion boundary"

jq -e '
  .promotion.next_blocks == ["STD-SYNC-COLLECTION-TEST-001"]
  and (.promotion.remaining | index("STD-SYNC-COLLECTION-ITER-001")) == null
  and (.promotion.remaining | index("STD-SYNC-COLLECTION-TEST-001")) != null
' "$root/testing/stdlib-sync-collection.json" >/dev/null \
    || die "collection implementation registry does not expose the iteration leaf"

grep -Fq 'STD-SYNC-COLLECTION-ITER-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the iteration leaf"
grep -Fq 'stdlib-sync-collection-iter.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the iteration contract"
grep -Fq 'stdlib-sync-collection-iter.md' "$root/docs/contracts/stdlib-sync.md" \
    || die "parent sync document does not link the iteration contract"

echo "std.sync collection iteration: OK (hosted cursor; native private ABI; finite generation horizon)"
