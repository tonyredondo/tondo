#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="$(printenv TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT || printf '%s/testing/stdlib-sync-conformance.json' "$root")"

die() {
    echo "std.sync conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-sync-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.sync"
  and .task == "STD-SYNC-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-sync.json"
  and .document == "docs/contracts/stdlib-sync-conformance.md"
  and .vm.fixture == "tests/runtime/m11-std-sync-conformance-001.to"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 9)
  and .vm.expected_stdout[0] == "atomic-orders:1:4"
  and .vm.expected_stdout[1] == "compare-exchange:cas"
  and .vm.expected_stdout[2] == "parking-wakeup:notify"
  and .vm.expected_stdout[3] == "cleanup-no-poison:guards-permits"
  and .vm.expected_stdout[4] == "once-publication:41"
  and .vm.expected_stdout[5] == "barrier-generations:2"
  and .vm.expected_stdout[6] == "threads-capability:static-rejection"
  and .vm.expected_stdout[7] == "collection-conformance:delegated"
  and .vm.expected_stdout[8] == "sync-conformance-ok"
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-sync-lowering"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.observable == "ordered-vm-lines-plus-native-normalized-result-tags"
  and .rules.native_scope == "private-atomic-parking-worker-bridge; locks-once-barrier-are-vm-owned"
  and .rules.capability == "cross-thread-sharing-and-spawn-require-threads; cooperative-sync-does-not"
  and .rules.collection_dependency == "STD-SYNC-COLLECTION-CONF-001-runs-as-a-required-child-conformance"
  and .rules.static_rejections == "missing-threads-capability-is-a-driver-and-compile-fail-rejection"
  and .rules.cleanup == "fresh-case-reset-and-zero-live-objects-before-return"
  and .rules.native_aot == "not-claimed"
  and (.cases | length == 8)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .native_expected.status == "passed" and .native_expected.cleanup == true)
  and .cases[0].native_expected == {id:"atomic-orders",status:"passed",orders:[0,1,2,3,4],invalid_order:true,cleanup:true}
  and .cases[1].native_expected == {id:"compare-exchange",status:"passed",final:40,workers:4,increments:40,mismatch:true,cleanup:true}
  and .cases[2].native_expected == {id:"parking-wakeup",status:"passed",one:true,all:true,timeout:true,epoch:2,cleanup:true}
  and .cases[3].native_expected == {id:"cleanup-no-poison",status:"passed",shared_arc:true,double_release:true,live_objects:0,cleanup:true}
  and .cases[4].native_expected == {id:"once-publication",status:"passed",published:41,retry:true,memoized:true,native_scope:"atomic-publication-bridge",cleanup:true}
  and .cases[5].native_expected == {id:"barrier-generations",status:"passed",generations:2,epoch:2,native_scope:"epoch-parking-bridge",cleanup:true}
  and .cases[6].native_expected == {id:"threads-capability",status:"passed",completed:true,cancelled:true,distinct_worker:true,cleanup:true}
  and .cases[7].native_expected == {id:"collection-conformance",status:"passed",delegated:"STD-SYNC-COLLECTION-CONF-001",cleanup:true}
  and (.negative_cases | length == 16)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and all([.vm.fixture, .native.runtime, .native.probe, .contract, .document][]; startswith("/") | not)
  and .report == "target/reliability/evidence/stdlib-sync-conformance.json"
  and .next_blocks == ["STD-SYNC-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in testing/stdlib-sync.json testing/stdlib-sync-test.json testing/stdlib-sync-collection-conformance.json docs/contracts/stdlib-sync.md docs/contracts/stdlib-sync-conformance.md docs/contracts/stdlib-sync-collection-conformance.md TONDO_STANDARD_LIBRARY_SPEC.md TONDO_LANGUAGE_SPEC.md TONDO_IMPLEMENTATION_TRACKER.md tests/runtime/m11-std-sync-conformance-001.to tests/runtime/m11-std-sync-conformance-001.stdout tests/runtime/m11-std-sync-conformance-001.exit tests/compile-fail/m11-std-sync-conf-missing-threads.to tests/compile-fail/m11-std-sync-conf-missing-threads.codes crates/tondo-compiler/src/driver.rs crates/tondo-native-runtime/src/lib.rs crates/tondo-native-runtime/examples/sync_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in scripts/stdlib-sync-conformance-check.sh scripts/stdlib-sync-conformance-test.sh scripts/stdlib-sync-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for symbol in tondo_rt_atomic_new tondo_rt_atomic_load tondo_rt_atomic_store tondo_rt_atomic_swap tondo_rt_atomic_compare_exchange tondo_rt_sync_park_new tondo_rt_sync_park_wait tondo_rt_sync_park_wake tondo_rt_sync_park_waiters tondo_rt_thread_spawn tondo_rt_thread_worker_wait tondo_rt_live_objects tondo_rt_mark_shared tondo_rt_sync_array_get; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" || die "native conformance symbol is missing: $symbol"
done

for test in crates/tondo-compiler/src/driver.rs::sync_bootstrap_surface_is_lowered_when_the_module_is_imported crates/tondo-compiler/src/driver.rs::direct_suspension_is_inferred_and_join_can_cross_a_function_boundary crates/tondo-compiler/src/driver.rs::thread_spawn_requires_an_explicit_threads_target_capability crates/tondo-native-runtime/src/lib.rs::native_sync_atomics_are_linearizable_across_threads crates/tondo-native-runtime/src/lib.rs::native_thread_uses_a_distinct_worker_and_join_waits_for_completion; do
    file="${test%%::*}"
    name="${test##*::}"
    grep -Fq "$name" "$root/$file" || die "missing static/runtime test anchor: $test"
done

for marker in 'same eight logical cases' 'complete source-level surface' 'private ABI' 'native AOT' 'collection case consumes' 'zero live objects' 'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-sync-conformance.md" || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-SYNC-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-sync-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-sync-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-SYNC-DOC-001"]
  and (.promotion.remaining | index("STD-SYNC-CONF-001")) == null
' "$root/testing/stdlib-sync.json" >/dev/null || die "std.sync owner registry does not promote sync conformance"

jq -e '
  .collections.conformance.task == "STD-SYNC-COLLECTION-CONF-001"
  and .collections.conformance.status == "verified"
  and .collections.conformance.contract == "testing/stdlib-sync-collection-conformance.json"
  and .promotion.next_blocks == ["STD-SYNC-DOC-001"]
' "$root/testing/stdlib-sync.json" >/dev/null || die "std.sync registry does not retain the collection dependency"

jq -e '.next_blocks == ["STD-SYNC-DOC-001"]' "$root/testing/stdlib-sync-collection-conformance.json" >/dev/null || die "collection child conformance does not advance with its owner"

grep -Fxq 'E1008' "$root/tests/compile-fail/m11-std-sync-conf-missing-threads.codes" || die "missing-threads fixture does not pin E1008"
grep -Fq 'stdlib-sync-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" || die "main stdlib spec does not link sync conformance"
grep -Fq 'stdlib-sync-conformance.md' "$root/docs/contracts/stdlib-sync.md" || die "parent sync document does not link sync conformance"
grep -Fq 'STD-SYNC-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" || die "tracker does not record sync conformance"

echo "std.sync conformance contract: OK (8 shared cases; hosted VM/native bridge; collection dependency)"
