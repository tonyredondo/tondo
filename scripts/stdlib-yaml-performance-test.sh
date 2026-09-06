#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

tmp_base="$(printenv TMPDIR || printf '%s' /tmp)"
tmp_dir="$(mktemp -d "$tmp_base/tondo-stdlib-yaml-performance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_STDLIB_YAML_PERF_CONTRACT="$candidate" \
        scripts/stdlib-yaml-performance-check.sh >/dev/null 2>&1; then
        echo "std.yaml performance contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/open-status.json"
expect_failure open-status "$tmp_dir/open-status.json"

jq '.protocol.minimum_sample_count = 26' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/sample-count.json"
expect_failure minimum-sample-count "$tmp_dir/sample-count.json"

jq '.workloads[1].id = .workloads[0].id' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/duplicate-workload.json"
expect_failure duplicate-workload "$tmp_dir/duplicate-workload.json"

jq '.workloads[0].operation = "flat-only"' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/invalid-operation.json"
expect_failure invalid-operation "$tmp_dir/invalid-operation.json"

jq '.workloads[6].expected_error = null' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/missing-adversarial-error.json"
expect_failure missing-adversarial-error "$tmp_dir/missing-adversarial-error.json"

jq '.probe.sha256 = ("0" * 64)' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/probe-hash.json"
expect_failure probe-hash "$tmp_dir/probe-hash.json"

jq '.strategy.native_aot = "verified"' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/native-aot-claim.json"
expect_failure native-aot-claim "$tmp_dir/native-aot-claim.json"

jq '.metrics = ["throughput"]' testing/stdlib-yaml-performance.json \
    > "$tmp_dir/missing-metrics.json"
expect_failure missing-metrics "$tmp_dir/missing-metrics.json"

jq '.report = "target/reliability/evidence/other.json"' \
    testing/stdlib-yaml-performance.json > "$tmp_dir/report-path.json"
expect_failure report-path "$tmp_dir/report-path.json"

TONDO_STDLIB_YAML_PERF_ALLOW_DIRTY=1 \
    scripts/stdlib-yaml-performance.sh >/dev/null

report_dir="${TONDO_STDLIB_YAML_PERF_EVIDENCE_DIR:-target/reliability/evidence}"
report="$report_dir/stdlib-yaml-performance.json"
[[ -s "$report" ]] || {
    echo "std.yaml performance contract tests: runner did not write $report" >&2
    exit 1
}

echo "std.yaml performance contract tests: OK (status, protocol, workloads, strategy, oracle, report and runner negatives)"
