#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_COLLECTION_CONTRACT:-$root/testing/stdlib-sync-collection.json}"

die() {
    echo "std.sync collection implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing implementation contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.sync.collection"
  and .parent_owner == "std.sync"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-SYNC-COLLECTION-IMPL-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-sync-collection.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-sync.json"
  and .frontend_contract == "testing/stdlib-sync-collection-frontend.json"
  and .layer == "B2"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native-runtime-abi"
  and (.surface.identities | length) == 5
  and .surface.handle_traits == "Copy + Discard + Send + Share"
  and (.surface.constructors | length) == 5
  and (.surface.methods | length) == 32
  and .surface.array.length == "fixed"
  and .surface.array.index == "stable"
  and .surface.array.structural_mutation == "forbidden"
  and .surface.map.ordering == "insertion-by-linearization"
  and .surface.map.replace_moves_key == false
  and .surface.set.ordering == "insertion-by-linearization"
  and .surface.stack.ordering == "LIFO"
  and .surface.queue.ordering == "FIFO-MPMC"
  and .surface.selectable == false
  and .surface.operations_suspend_under_contention == true
  and .surface.snapshot == "one-linearization-coherent-value-collection"
  and .ownership.handle_copy == "copies-identity"
  and .ownership.handle_clone == false
  and .ownership.discard == "release-exactly-once"
  and .ownership.cow_clone == "shares-the-synchronization-cell"
  and .ownership.reclamation == "handle-table-cleanup-plus-Arc-cell-drop"
  and .ownership.stale_handle == "fail-closed-before-cell-access"
  and .compare_exchange.strength == "strong-no-spurious-failure"
  and .compare_exchange.mismatch == "observed-value-without-write"
  and .runtime.hosted_vm.status == "verified"
  and .runtime.hosted_vm.model == "single-worker-ready-job-linearization"
  and .runtime.native_runtime_abi.status == "verified"
  and .runtime.native_runtime_abi.carrier == "opaque-u64-capability"
  and .runtime.native_runtime_abi.storage == "per-collection-RwLock-or-Mutex-cell"
  and .runtime.native_runtime_abi.parking == "per-collection-epoch-Condvar"
  and .runtime.native_runtime_abi.global_lock_while_waiting == false
  and .runtime.native_runtime_abi.blocking == "native-workers-only"
  and .runtime.native_aot_lowering == "not-claimed"
  and .runtime.public_api_promoted == false
  and .limits.unbounded_growth == "forbidden"
  and .implementation.status == "verified-hosted-vm-and-native-runtime-abi"
  and .implementation.public_api_promoted == false
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.algorithmic_fast_paths == "deferred-to-STD-SYNC-COLLECTION-PERF-001"
  and (.implementation.sources | type == "array" and length == 11)
  and (.implementation.tests | type == "array" and length == 7)
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == ["STD-SYNC-CONF-001", "STD-SYNC-DOC-001"]
  and .conformance.task == "STD-SYNC-COLLECTION-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-sync-collection-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-sync-collection-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.cases == 8
  and .conformance.vm_lines == 9
  and .conformance.native_status == "verified-native-runtime-abi"
  and .conformance.native_aot == "not-claimed"
  and .conformance.report == "target/reliability/evidence/stdlib-sync-collection-conformance.json"
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 14
  and .promotion.implementation_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-SYNC-CONF-001"]
  and .promotion.remaining == .implementation.required_follow_ups
' "$contract" >/dev/null || die "invalid machine-readable implementation contract"

for path in \
    docs/contracts/stdlib-sync-collection.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json \
    testing/stdlib-sync-collection-frontend.json \
    docs/contracts/native-abi.md \
    testing/stdlib-sync-collection-iter.json \
    docs/contracts/stdlib-sync-collection-iter.md \
    testing/stdlib-sync-collection-performance.json \
    docs/contracts/stdlib-sync-collection-performance.md \
    testing/stdlib-sync-collection-conformance.json \
    docs/contracts/stdlib-sync-collection-conformance.md; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")

while IFS= read -r test; do
    if [[ "$test" == scripts/* ]]; then
        [[ -x "$root/$test" ]] || die "implementation test script is not executable: $test"
        continue
    fi
    file="${test%%::*}"
    name="${test##*::}"
    [[ -f "$root/$file" ]] || die "missing test source: $file"
    grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
done < <(jq -r '.implementation.tests[]' "$contract")

for marker in \
    'STD-SYNC-COLLECTION-IMPL-001' \
    'opaque-u64-capability' \
    'single-worker-ready-job-linearization' \
    'per-identity RwLock cells' \
    'epoch `Condvar` parking signal' \
    'strong and' \
    'snapshot' \
    'native AOT lowering' \
    'STD-SYNC-COLLECTION-ITER-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-sync-collection.md" \
        || die "implementation document misses marker: $marker"
done

for symbol in \
    tondo_rt_sync_array_new \
    tondo_rt_sync_map_new \
    tondo_rt_sync_set_new \
    tondo_rt_sync_stack_new \
    tondo_rt_sync_queue_new \
    tondo_rt_sync_array_compare_exchange \
    tondo_rt_sync_map_compare_exchange \
    tondo_rt_sync_array_snapshot \
    tondo_rt_sync_map_snapshot \
    tondo_rt_sync_set_snapshot \
    tondo_rt_sync_stack_snapshot \
    tondo_rt_sync_queue_snapshot; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native collection symbol is missing: $symbol"
done
for marker in \
    'native_sync_read' \
    'native_sync_write' \
    'native_sync_mutex' \
    'sync_collection_parks' \
    'RESULT_CAS_EXCHANGED' \
    'STATUS_COLLECTION_INVALID'; do
    grep -Fq "$marker" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native collection implementation anchor is missing: $marker"
done

for marker in \
    'SyncArrayLiteral' \
    'SyncMapLiteral' \
    'SyncSetLiteral' \
    'SyncStackLiteral' \
    'SyncQueueLiteral' \
    'SyncArraySnapshot' \
    'SyncQueueSnapshot'; do
    grep -Fq "$marker" \
        "$root/crates/tondo-compiler/src/hir.rs" \
        "$root/crates/tondo-compiler/src/process_host.rs" \
        || die "compiler/host collection anchor is missing: $marker"
done

jq -e '
  .frontend.task == "STD-SYNC-COLLECTION-FRONTEND-001"
  and .frontend.runtime_lowering == "verified-hosted-runtime-boundary"
  and .frontend.implementation_contract == "testing/stdlib-sync-collection.json"
  and .collections.implementation_contract == "testing/stdlib-sync-collection.json"
  and .collections.conformance.task == "STD-SYNC-COLLECTION-CONF-001"
  and .collections.conformance.status == "verified"
  and .collections.conformance.contract == "testing/stdlib-sync-collection-conformance.json"
  and .collections.conformance.document == "docs/contracts/stdlib-sync-collection-conformance.md"
  and .collections.conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-SYNC-CONF-001"]
  and (.promotion.implementation_pending | index("STD-SYNC-COLLECTION-IMPL-001")) == null
' "$root/testing/stdlib-sync.json" >/dev/null \
    || die "parent std.sync registry does not promote the implementation leaf"

jq -e '
  .implementation.status == "verified-frontend-lowering-consumed"
  and .implementation.mir_boundary == "verified-hosted-runtime-boundary"
  and .implementation.runtime == "verified-by-STD-SYNC-COLLECTION-IMPL-001"
  and .promotion.next_blocks == ["STD-SYNC-CONF-001"]
' "$root/testing/stdlib-sync-collection-frontend.json" >/dev/null \
    || die "frontend registry does not point at the consumed implementation boundary"

grep -Fq 'testing/stdlib-sync-collection.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the implementation contract"
grep -Fq 'stdlib-sync-collection.md' "$root/docs/contracts/stdlib-sync.md" \
    || die "parent sync document does not link the implementation contract"
grep -Fq 'stdlib-sync-collection-conformance.md' "$root/docs/contracts/stdlib-sync.md" \
    || die "parent sync document does not link the collection conformance contract"
grep -Fq 'STD-SYNC-COLLECTION-IMPL-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the implementation leaf"

echo "std.sync collection implementation: OK (hosted VM; native ABI; bounded ordered handles)"
