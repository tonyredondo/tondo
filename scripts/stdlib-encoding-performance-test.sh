#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

tmp_base="$(printenv TMPDIR || printf '%s' /tmp)"
tmp_dir="$(mktemp -d "$tmp_base/tondo-stdlib-encoding-performance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_STDLIB_ENCODING_PERF_CONTRACT="$candidate" \
        scripts/stdlib-encoding-performance-check.sh >/dev/null 2>&1; then
        echo "std.encoding performance contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/open-status.json"
expect_failure open-status "$tmp_dir/open-status.json"

jq '.protocol.minimum_sample_count = 26' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/sample-count.json"
expect_failure minimum-sample-count "$tmp_dir/sample-count.json"

jq '.workloads[1].id = .workloads[0].id' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/duplicate-workload.json"
expect_failure duplicate-workload "$tmp_dir/duplicate-workload.json"

jq '.workloads[0].size_class = "medium"' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/invalid-size-class.json"
expect_failure invalid-size-class "$tmp_dir/invalid-size-class.json"

jq '.probe.sha256 = ("0" * 64)' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/probe-hash.json"
expect_failure probe-hash "$tmp_dir/probe-hash.json"

jq '.strategy.native_aot = "verified"' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/native-aot-claim.json"
expect_failure native-aot-claim "$tmp_dir/native-aot-claim.json"

jq '.strategy.multiversion_dispatch = "simd-selected"' \
    testing/stdlib-encoding-performance.json > "$tmp_dir/multiversion-claim.json"
expect_failure multiversion-claim "$tmp_dir/multiversion-claim.json"

jq '.oracle.sources = []' testing/stdlib-encoding-performance.json \
    > "$tmp_dir/missing-oracle.json"
expect_failure missing-oracle "$tmp_dir/missing-oracle.json"

jq '.report = "target/reliability/evidence/other.json"' \
    testing/stdlib-encoding-performance.json > "$tmp_dir/report-path.json"
expect_failure report-path "$tmp_dir/report-path.json"

TONDO_STDLIB_ENCODING_PERF_ALLOW_DIRTY=1 \
    scripts/stdlib-encoding-performance.sh >/dev/null

report_dir="$(printenv TONDO_STDLIB_ENCODING_PERF_EVIDENCE_DIR || printf '%s' target/reliability/evidence)"
report="$report_dir/stdlib-encoding-performance.json"
[[ -s "$report" ]] || {
    echo "std.encoding performance contract tests: runner did not write $report" >&2
    exit 1
}

echo "std.encoding performance contract tests: OK (status, protocol, workload, strategy, oracle, report and runner negatives)"
