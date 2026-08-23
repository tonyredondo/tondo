#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "$root/.tmp/tondo-async-select-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local contract="$2"
    if TONDO_SELECT_PERF_ALLOW_DIRTY=1 \
        TONDO_SELECT_PERF_CONTRACT="$contract" \
        scripts/async-select-performance.sh >/dev/null 2>&1; then
        echo "async select performance contract test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.protocol.minimum_sample_count = 26' \
    testing/async-select-performance.json > "$tmp/sample-count.json"
expect_failure minimum-sample-count "$tmp/sample-count.json"

jq '.workloads[0].arms = 0' \
    testing/async-select-performance.json > "$tmp/zero-arms.json"
expect_failure zero-arms "$tmp/zero-arms.json"

jq '.probe.sha256 = ("0" * 64)' \
    testing/async-select-performance.json > "$tmp/probe-hash-mismatch.json"
expect_failure probe-hash-mismatch "$tmp/probe-hash-mismatch.json"

jq '.workloads |= .[0:8]' \
    testing/async-select-performance.json > "$tmp/missing-workload.json"
expect_failure missing-workload "$tmp/missing-workload.json"

echo "async select performance contract tests: OK"
