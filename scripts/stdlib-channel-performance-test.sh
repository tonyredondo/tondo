#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-channel-performance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_STDLIB_CHANNEL_PERF_CONTRACT="$candidate" \
        scripts/stdlib-channel-performance-check.sh >/dev/null 2>&1; then
        echo "std.channel performance contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-channel-performance.json \
    > "$tmp_dir/open-status.json"
expect_failure open-status "$tmp_dir/open-status.json"

jq '.protocol.minimum_sample_count = 26' testing/stdlib-channel-performance.json \
    > "$tmp_dir/sample-count.json"
expect_failure minimum-sample-count "$tmp_dir/sample-count.json"

jq '.workloads[1].id = .workloads[0].id' testing/stdlib-channel-performance.json \
    > "$tmp_dir/duplicate-workload.json"
expect_failure duplicate-workload "$tmp_dir/duplicate-workload.json"

jq '.workloads[0].topology = "many"' testing/stdlib-channel-performance.json \
    > "$tmp_dir/unsupported-topology.json"
expect_failure unsupported-topology "$tmp_dir/unsupported-topology.json"

jq '.workloads[0].capacity = 2' testing/stdlib-channel-performance.json \
    > "$tmp_dir/unsupported-capacity.json"
expect_failure unsupported-capacity "$tmp_dir/unsupported-capacity.json"

jq '.probe.sha256 = ("0" * 64)' testing/stdlib-channel-performance.json \
    > "$tmp_dir/probe-hash.json"
expect_failure probe-hash "$tmp_dir/probe-hash.json"

jq '.strategy.native_aot = "verified"' testing/stdlib-channel-performance.json \
    > "$tmp_dir/native-aot-claim.json"
expect_failure native-aot-claim "$tmp_dir/native-aot-claim.json"

jq '.strategy.algorithmic_fast_paths = "verified-lock-free"' \
    testing/stdlib-channel-performance.json > "$tmp_dir/fast-path-claim.json"
expect_failure fast-path-claim "$tmp_dir/fast-path-claim.json"

jq '.invariants.fifo = "completion-order"' testing/stdlib-channel-performance.json \
    > "$tmp_dir/fifo-invariant.json"
expect_failure fifo-invariant "$tmp_dir/fifo-invariant.json"

jq '.oracle.sources = []' testing/stdlib-channel-performance.json \
    > "$tmp_dir/missing-oracle.json"
expect_failure missing-oracle "$tmp_dir/missing-oracle.json"

jq '.report = "target/reliability/evidence/other.json"' \
    testing/stdlib-channel-performance.json > "$tmp_dir/report-path.json"
expect_failure report-path "$tmp_dir/report-path.json"

jq '.workloads |= .[0:8]' testing/stdlib-channel-performance.json \
    > "$tmp_dir/missing-workload.json"
expect_failure missing-workload "$tmp_dir/missing-workload.json"

echo "std.channel performance contract tests: OK (negative status, protocol, topology, workload, strategy, oracle and report cases)"
