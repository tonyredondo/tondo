#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT:-$root/testing/stdlib-sync-collection-conformance.json}"

die() {
    echo "std.sync collection conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-sync-collection-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.sync.collection"
  and .task == "STD-SYNC-COLLECTION-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-sync-collection.json"
  and .document == "docs/contracts/stdlib-sync-collection-conformance.md"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 9)
  and .vm.expected_stdout[0] == "array-alias-bounds:3:9"
  and .vm.expected_stdout[1] == "map-linearization:25:2"
  and .vm.expected_stdout[2] == "set-linearization:21:2"
  and .vm.expected_stdout[3] == "stack-queue-order:321:234:3"
  and .vm.expected_stdout[4] == "cursor-horizon:12:12:12:21:12"
  and .vm.expected_stdout[5] == "snapshot-equivalence:9"
  and .vm.expected_stdout[6] == "limits-cleanup:empty-none"
  and .vm.expected_stdout[7] == "threads-capability:shared-alias"
  and .vm.expected_stdout[8] == "sync-collection-conformance-ok"
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-collection-lowering"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.observable == "exact-ordered-lines-plus-native-result-tags"
  and .rules.literal_alias == "copy-handle-same-state"
  and .rules.bounds == "recoverable-none-or-CollectionError"
  and .rules.linearization == "one-operation-at-a-time-and-insertion-order"
  and .rules.direct_for == "finite-weakly-consistent-value-only-no-body-lock"
  and .rules.snapshot == "one-linearization-coherent-value-collection"
  and .rules.stack_queue == "non-destructive-observation"
  and .rules.capability == "cross-thread-sharing-requires-threads; cooperative-hosted-does-not"
  and .rules.suspension == "inferred-for-suspendable-collection-operations"
  and .rules.static_rejections == "ref-mut-var-bindings-and-missing-stack-queue-bounds"
  and .rules.cleanup == "fresh-case-reset-and-zero-live-objects-before-return"
  and .rules.native_aot == "not-claimed"
  and (.cases | length == 8)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .vm_observable == .native_expected.line and .native_expected.status == "passed" and .native_expected.cleanup == true)
  and .cases[0].native_expected == {status:"passed",line:"array-alias-bounds:3:9",cleanup:true}
  and .cases[1].native_expected == {status:"passed",line:"map-linearization:25:2",cleanup:true}
  and .cases[2].native_expected == {status:"passed",line:"set-linearization:21:2",cleanup:true}
  and .cases[3].native_expected == {status:"passed",line:"stack-queue-order:321:234:3",cleanup:true}
  and .cases[4].native_expected == {status:"passed",line:"cursor-horizon:12:12:12:21:12",cleanup:true}
  and .cases[5].native_expected == {status:"passed",line:"snapshot-equivalence:9",cleanup:true}
  and .cases[6].native_expected == {status:"passed",line:"limits-cleanup:empty-none",cleanup:true}
  and .cases[7].native_expected == {status:"passed",line:"threads-capability:shared-alias",cleanup:true,workers:4,increments:40}
  and (.negative_cases | length == 13)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and .report == "target/reliability/evidence/stdlib-sync-collection-conformance.json"
  and .next_blocks == ["STD-SYNC-CONF-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-sync-collection.json \
    docs/contracts/stdlib-sync-collection-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json \
    tests/runtime/m11-std-sync-collection-conformance-001.to \
    tests/runtime/m11-std-sync-collection-conformance-001.stdout \
    tests/runtime/m11-std-sync-collection-conformance-001.exit \
    crates/tondo-native-runtime/src/lib.rs \
    crates/tondo-native-runtime/examples/sync_collection_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in \
    scripts/stdlib-sync-collection-conformance-check.sh \
    scripts/stdlib-sync-collection-conformance-test.sh \
    scripts/stdlib-sync-collection-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for symbol in \
    tondo_rt_sync_cursor_start \
    tondo_rt_sync_cursor_next \
    tondo_rt_sync_cursor_key \
    tondo_rt_sync_array_compare_exchange \
    tondo_rt_sync_map_compare_exchange \
    tondo_rt_sync_array_snapshot \
    tondo_rt_sync_map_snapshot \
    tondo_rt_sync_set_snapshot \
    tondo_rt_sync_stack_snapshot \
    tondo_rt_sync_queue_snapshot \
    tondo_rt_live_objects \
    tondo_rt_mark_shared; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native conformance symbol is missing: $symbol"
done

for test in \
    'crates/tondo-compiler/src/driver.rs::sync_collection_surface_executes_through_the_hosted_vm' \
    'crates/tondo-compiler/src/driver.rs::sync_collection_direct_for_uses_finite_host_cursor_order' \
    'crates/tondo-compiler/src/hir/check.rs::sync_collection_direct_iteration_is_value_only_and_suspendable' \
    'crates/tondo-compiler/src/process_host.rs::sync_collection_cursor_preserves_order_horizon_and_reinsertion_boundary' \
    'crates/tondo-native-runtime/src/lib.rs::native_sync_cursor_is_finite_ordered_and_generation_safe' \
    'crates/tondo-native-runtime/src/lib.rs::native_sync_collections_cover_empty_and_invalid_edges'; do
    file="${test%%::*}"
    name="${test##*::}"
    grep -Fq "$name" "$root/$file" || die "missing static/runtime test anchor: $test"
done

for marker in \
    'same eight logical cases' \
    'same observable lines' \
    'finite structural horizon' \
    'threads' \
    'native AOT' \
    'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-sync-collection-conformance.md" \
        || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-SYNC-COLLECTION-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-sync-collection-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-sync-collection-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-SYNC-CONF-001"]
  and (.promotion.remaining | index("STD-SYNC-COLLECTION-CONF-001")) == null
' "$root/testing/stdlib-sync-collection.json" >/dev/null \
    || die "collection implementation registry does not promote conformance"

jq -e '
  .collections.conformance.task == "STD-SYNC-COLLECTION-CONF-001"
  and .collections.conformance.status == "verified"
  and .collections.conformance.contract == "testing/stdlib-sync-collection-conformance.json"
  and .collections.conformance.document == "docs/contracts/stdlib-sync-collection-conformance.md"
  and .collections.conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-SYNC-CONF-001"]
' "$root/testing/stdlib-sync.json" >/dev/null \
    || die "parent std.sync registry does not expose collection conformance"

grep -Fq 'stdlib-sync-collection-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link collection conformance"
grep -Fq 'stdlib-sync-collection-conformance.md' "$root/docs/contracts/stdlib-sync.md" \
    || die "parent sync document does not link collection conformance"
grep -Fq 'STD-SYNC-COLLECTION-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record collection conformance"

echo "std.sync collection conformance contract: OK (8 shared cases; hosted VM/native ABI; static boundary)"
