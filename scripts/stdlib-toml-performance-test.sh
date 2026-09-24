#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_contract_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_STDLIB_TOML_PERF_CONTRACT="$candidate" \
        scripts/stdlib-toml-performance-check.sh >/dev/null 2>&1; then
        echo "std.toml performance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-toml-performance.json > "$tmp/open.json"
expect_contract_failure open "$tmp/open.json"
jq '.protocol.minimum_sample_count = 26' testing/stdlib-toml-performance.json > "$tmp/samples.json"
expect_contract_failure samples "$tmp/samples.json"
jq '.workloads[1].id = .workloads[0].id' testing/stdlib-toml-performance.json > "$tmp/duplicate.json"
expect_contract_failure duplicate "$tmp/duplicate.json"
jq '.workloads[9].expected_error = null' testing/stdlib-toml-performance.json > "$tmp/rejection.json"
expect_contract_failure rejection "$tmp/rejection.json"
jq '.probe.sha256 = ("0" * 64)' testing/stdlib-toml-performance.json > "$tmp/probe.json"
expect_contract_failure probe "$tmp/probe.json"
jq '.strategy.hosted_vm = "verified"' testing/stdlib-toml-performance.json > "$tmp/hosted.json"
expect_contract_failure hosted-claim "$tmp/hosted.json"
jq '.strategy.native_aot = "verified"' testing/stdlib-toml-performance.json > "$tmp/native.json"
expect_contract_failure native-claim "$tmp/native.json"
jq '.metrics = ["throughput"]' testing/stdlib-toml-performance.json > "$tmp/metrics.json"
expect_contract_failure metrics "$tmp/metrics.json"
jq '.report = "target/other.json"' testing/stdlib-toml-performance.json > "$tmp/report.json"
expect_contract_failure report "$tmp/report.json"

TONDO_STDLIB_TOML_PERF_ALLOW_DIRTY=1 \
    TONDO_STDLIB_TOML_PERF_EVIDENCE_DIR="$tmp/evidence" \
    scripts/stdlib-toml-performance.sh >/dev/null
report="$tmp/evidence/stdlib-toml-performance.json"
[[ -s "$report" ]] || { echo "std.toml performance tests: report missing" >&2; exit 1; }

expect_report_failure() {
    local name="$1"
    local candidate="$2"
    if python3 -B scripts/stdlib_toml_performance.py check-report \
        --contract testing/stdlib-toml-performance.json --report "$candidate" \
        >/dev/null 2>&1; then
        echo "std.toml performance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.measurements[0].samples_ns |= .[:-1]' "$report" > "$tmp/missing-sample.json"
expect_report_failure missing-sample "$tmp/missing-sample.json"
jq '.measurements[0].p99_ns = 0' "$report" > "$tmp/percentile.json"
expect_report_failure percentile "$tmp/percentile.json"
jq '.measurements[0].counters.live_handles = 1' "$report" > "$tmp/live-handle.json"
expect_report_failure live-handle "$tmp/live-handle.json"
jq '.measurements[0].throughput_bytes_per_second = 0' "$report" > "$tmp/throughput.json"
expect_report_failure throughput "$tmp/throughput.json"
jq '.measurements[0].identity_sha256 = ("0" * 64)' "$report" > "$tmp/identity.json"
expect_report_failure identity "$tmp/identity.json"
jq '.strategy.native_aot = "verified"' "$report" > "$tmp/native-report.json"
expect_report_failure native-report "$tmp/native-report.json"

echo "std.toml performance tests: OK (contract, provenance, report, lifecycle and strategy negatives)"
