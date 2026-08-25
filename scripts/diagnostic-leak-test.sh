#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-leak-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_LEAK_CONTRACT="$candidate" scripts/diagnostic-leak-check.sh >/dev/null 2>&1; then
        echo "diagnostic leak tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' testing/diagnostic-leak.json > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.algorithm.heap = "reference-counting"' testing/diagnostic-leak.json > "$tmp/wrong-heap.json"
expect_failure wrong-heap "$tmp/wrong-heap.json"

jq '.limits.min_growth_snapshots = 1' testing/diagnostic-leak.json > "$tmp/too-few-snapshots.json"
expect_failure too-few-snapshots "$tmp/too-few-snapshots.json"

jq '.limits.max_findings = 0' testing/diagnostic-leak.json > "$tmp/invalid-budget.json"
expect_failure invalid-budget "$tmp/invalid-budget.json"

jq '.lifecycle.fresh_process_per_attempt = false' \
    testing/diagnostic-leak.json > "$tmp/shared-attempt-state.json"
expect_failure shared-attempt-state "$tmp/shared-attempt-state.json"

jq '.public_stdlib_api = true' testing/diagnostic-leak.json > "$tmp/public-api.json"
expect_failure public-api "$tmp/public-api.json"

jq '.next_blocks = ["LEAK-001", "DUMP-001"]' \
    testing/diagnostic-leak.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

echo "diagnostic leak tests: OK (status, GC model, snapshots, budgets, lifecycle and privacy negatives rejected)"
