#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-async-group-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local contract="$2"
    if TONDO_ASYNC_GROUP_PERF_ALLOW_DIRTY=1 \
        TONDO_ASYNC_GROUP_PERF_CONTRACT="$contract" \
        scripts/stdlib-async-group-performance.sh >/dev/null 2>&1; then
        echo "async Group performance contract test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.protocol.minimum_sample_count = 26' \
    testing/stdlib-async-group-performance.json > "$tmp/sample-count.json"
expect_failure minimum-sample-count "$tmp/sample-count.json"

jq '.workloads[0].cardinality = 2' \
    testing/stdlib-async-group-performance.json > "$tmp/unsupported-cardinality.json"
expect_failure unsupported-cardinality "$tmp/unsupported-cardinality.json"

jq '.probe.sha256 = ("0" * 64)' \
    testing/stdlib-async-group-performance.json > "$tmp/probe-hash-mismatch.json"
expect_failure probe-hash-mismatch "$tmp/probe-hash-mismatch.json"

jq '.workloads |= .[0:17]' \
    testing/stdlib-async-group-performance.json > "$tmp/missing-workload.json"
expect_failure missing-workload "$tmp/missing-workload.json"

jq '.invariants.wakeup = "unbounded"' \
    testing/stdlib-async-group-performance.json > "$tmp/wakeup-invariant.json"
expect_failure wakeup-invariant "$tmp/wakeup-invariant.json"

echo "async Group performance contract tests: OK"
