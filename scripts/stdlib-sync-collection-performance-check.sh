#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT:-$root/testing/stdlib-sync-collection-performance.json}"

die() {
    echo "std.sync collection performance contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
    .format == "tondo-stdlib-sync-collection-performance/1"
    and .edition == "0.1"
    and .phase == "STD-0.1B"
    and .task == "STD-SYNC-COLLECTION-PERF-001"
    and .owner == "std.sync.collection"
    and .status == "verified-hosted-vm-baseline"
    and .target == "tondo-vm-hosted"
    and .backend == "bytecode-vm"
    and .profile == "test"
    and (.probe.path | type == "string" and length > 0)
    and (.probe.test | type == "string" and length > 0)
    and (.probe.sha256 | test("^[0-9a-f]{64}$"))
    and .protocol.warmup_iterations == 3
    and .protocol.measurement_repetitions == 9
    and .protocol.independent_processes == 3
    and .protocol.minimum_sample_count == 27
    and .protocol.batch_operations == 16
    and .protocol.deterministic_seed == "tondo-stdlib-sync-collection-perf-0.1"
    and (.workloads | length) == 31
    and ([.workloads[].id] | unique | length) == 31
    and ([.workloads[].participants] | unique | sort) == [1, 8, 64]
    and ([.workloads[].cardinality] | unique | sort) == [1, 8, 64]
    and .strategy.native_aot == "not-claimed"
    and .strategy.algorithmic_fast_paths == "deferred-to-native-targeted-performance-campaign"
    and .invariants.cursor == "direct-next-has-no-content-materialization-or-visited-table-and-does-not-hold-body-lock"
    and .invariants.cleanup == "no-pending-collection-job-waiter-or-scheduler-queue-before-probe-return"
    and .oracle.kind == "independent-bounded-collection-model-and-host-invariant-checks"
    and (.oracle.sources | type == "array" and length == 2)
    and (.oracle.sources | index("crates/tondo-reliability/src/sync_collection_model.rs")) != null
    and (.oracle.sources | index("crates/tondo-reliability/tests/sync_collection_models.rs")) != null
    and .report == "target/reliability/evidence/stdlib-sync-collection-performance.json"
' "$contract" >/dev/null || die "invalid machine-readable performance contract"

probe_path="$root/$(jq -r '.probe.path' "$contract")"
[[ -f "$probe_path" ]] || die "missing probe: ${probe_path#"$root"/}"
expected_probe_sha="$(jq -r '.probe.sha256' "$contract")"
actual_probe_sha="$(sha256sum "$probe_path" | cut -d' ' -f1)"
[[ "$actual_probe_sha" == "$expected_probe_sha" ]] || die "probe hash mismatch"

for path in \
    docs/contracts/stdlib-sync-collection-performance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-sync.json \
    testing/stdlib-sync-collection.json \
    testing/stdlib-sync-collection-iter.json \
    testing/stdlib-sync-collection-test.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

for path in \
    scripts/stdlib-sync-collection-performance-check.sh \
    scripts/stdlib-sync-collection-performance-test.sh \
    scripts/stdlib-sync-collection-performance.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'sync_collection_performance_probe' "$probe_path" \
    || die "probe test anchor is missing"
grep -Fq 'SYNC_COLLECTION_PERF_BATCH' "$probe_path" \
    || die "probe batch anchor is missing"
grep -Fq 'sync_collection_cursor_pass' "$probe_path" \
    || die "cursor workload anchor is missing"

cursor_source="$(sed -n '/    fn sync_cursor_start(/,/^    fn sync_cursor_next(/p' crates/tondo-compiler/src/process_host.rs)"
! grep -Fq 'HashSet' <<<"$cursor_source" \
    || die "cursor setup introduced a visited table"
! grep -Fq 'snapshot' <<<"$cursor_source" \
    || die "cursor setup materializes a snapshot"

jq -e '
  .implementation.algorithmic_fast_paths == "deferred-to-STD-SYNC-COLLECTION-PERF-001"
  and .runtime.native_aot_lowering == "not-claimed"
' testing/stdlib-sync-collection.json >/dev/null \
    || die "collection implementation contract no longer exposes the deferred fast-path boundary"
jq -e '
  .surface.materialization == "forbidden"
  and .runtime.native_aot_lowering == "not-claimed"
' testing/stdlib-sync-collection-iter.json >/dev/null \
    || die "collection iteration contract no longer exposes the no-materialization boundary"

jq -e '
  .collections.performance.task == "STD-SYNC-COLLECTION-PERF-001"
  and .collections.performance.contract == "testing/stdlib-sync-collection-performance.json"
  and .collections.performance.document == "docs/contracts/stdlib-sync-collection-performance.md"
  and .collections.performance.status == "verified-hosted-vm-baseline"
  and .collections.performance.target == "tondo-vm-hosted"
  and .collections.performance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-CHANNEL-IMPL-001"]
' testing/stdlib-sync.json >/dev/null \
    || die "parent std.sync registry does not expose the performance boundary"

jq -e '
  .performance.task == "STD-SYNC-COLLECTION-PERF-001"
  and .performance.contract == "testing/stdlib-sync-collection-performance.json"
  and .performance.document == "docs/contracts/stdlib-sync-collection-performance.md"
  and .performance.status == "verified-hosted-vm-baseline"
  and .promotion.next_blocks == ["STD-CHANNEL-IMPL-001"]
' testing/stdlib-sync-collection.json >/dev/null \
    || die "collection implementation registry does not expose the performance boundary"

for marker in \
    'STD-SYNC-COLLECTION-PERF-001' \
    'single-worker-ready-job-collection-baseline' \
    'algorithmic_fast_paths' \
    'direct-next-has-no-content-materialization-or-visited-table' \
    'target-qualified' \
    'native AOT'; do
    grep -Fq "$marker" docs/contracts/stdlib-sync-collection-performance.md \
        || die "performance document misses marker: $marker"
done
grep -Fq 'stdlib-sync-collection-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md \
    || die "stdlib spec does not link the collection performance contract"
grep -Fq 'stdlib-sync-collection-performance.md' docs/contracts/stdlib-sync.md \
    || die "parent sync document does not link the collection performance contract"
grep -Fq 'STD-SYNC-COLLECTION-PERF-001' TONDO_IMPLEMENTATION_TRACKER.md \
    || die "tracker does not record the collection performance leaf"

echo "std.sync collection performance contract: OK (hosted baseline; 31 workloads; deferred native fast paths explicit)"
