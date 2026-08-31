#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-sync-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local contract="$2"
    if TONDO_STDLIB_SYNC_PERF_ALLOW_DIRTY=1 \
        TONDO_STDLIB_SYNC_PERF_CONTRACT="$contract" \
        scripts/stdlib-sync-performance.sh >/dev/null 2>&1; then
        echo "std.sync performance contract test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.protocol.minimum_sample_count = 26' \
    testing/stdlib-sync-performance.json > "$tmp/sample-count.json"
expect_failure minimum-sample-count "$tmp/sample-count.json"

jq '.workloads[1].id = .workloads[0].id' \
    testing/stdlib-sync-performance.json > "$tmp/duplicate-workload.json"
expect_failure duplicate-workload "$tmp/duplicate-workload.json"

jq '.workloads[0].participants = 2' \
    testing/stdlib-sync-performance.json > "$tmp/unsupported-participants.json"
expect_failure unsupported-participants "$tmp/unsupported-participants.json"

jq '.probe.sha256 = ("0" * 64)' \
    testing/stdlib-sync-performance.json > "$tmp/probe-hash-mismatch.json"
expect_failure probe-hash-mismatch "$tmp/probe-hash-mismatch.json"

jq '.invariants.fairness = "allow-one-violation"' \
    testing/stdlib-sync-performance.json > "$tmp/fairness-budget.json"
expect_failure fairness-budget "$tmp/fairness-budget.json"

jq '.oracle.sources = []' \
    testing/stdlib-sync-performance.json > "$tmp/missing-oracle.json"
expect_failure missing-oracle "$tmp/missing-oracle.json"

jq '.report = "target/reliability/evidence/other.json"' \
    testing/stdlib-sync-performance.json > "$tmp/report-path.json"
expect_failure report-path "$tmp/report-path.json"

TONDO_STDLIB_SYNC_PERF_ALLOW_DIRTY=1 scripts/stdlib-sync-performance.sh >/dev/null

echo "std.sync performance contract tests: OK"
