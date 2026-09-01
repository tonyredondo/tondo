#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-collection-performance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync collection performance contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-sync-collection-performance.json > "$tmp_dir/open.json"
expect_failure open-status env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.protocol.minimum_sample_count = 26' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/sample-count.json"
expect_failure minimum-sample-count env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/sample-count.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.workloads[1].id = .workloads[0].id' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/duplicate-workload.json"
expect_failure duplicate-workload env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/duplicate-workload.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.workloads[0].participants = 2' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/unsupported-participants.json"
expect_failure unsupported-participants env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/unsupported-participants.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.probe.sha256 = ("0" * 64)' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/probe-hash.json"
expect_failure probe-hash env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/probe-hash.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.strategy.native_aot = "verified"' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/aot-claim.json"
expect_failure native-aot-claim env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.strategy.algorithmic_fast_paths = "verified-lock-free"' \
    testing/stdlib-sync-collection-performance.json > "$tmp_dir/fast-path-claim.json"
expect_failure fast-path-claim env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/fast-path-claim.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.invariants.cursor = "copy-all-values"' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/cursor-copy.json"
expect_failure cursor-copy env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/cursor-copy.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.oracle.sources = []' testing/stdlib-sync-collection-performance.json \
    > "$tmp_dir/missing-oracle.json"
expect_failure missing-oracle env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/missing-oracle.json" \
    scripts/stdlib-sync-collection-performance-check.sh

jq '.report = "target/reliability/evidence/other.json"' \
    testing/stdlib-sync-collection-performance.json > "$tmp_dir/report-path.json"
expect_failure report-path env \
    TONDO_STDLIB_SYNC_COLLECTION_PERF_CONTRACT="$tmp_dir/report-path.json" \
    scripts/stdlib-sync-collection-performance-check.sh

echo "std.sync collection performance contract tests: OK (negative status, protocol, workload, strategy, cursor, oracle and report cases)"
