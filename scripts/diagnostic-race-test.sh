#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-race-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_RACE_CONTRACT="$candidate" scripts/diagnostic-race-check.sh >/dev/null 2>&1; then
        echo "diagnostic race tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' testing/diagnostic-race.json > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.algorithm.clock = "lockset"' testing/diagnostic-race.json > "$tmp/wrong-clock.json"
expect_failure wrong-clock "$tmp/wrong-clock.json"

jq '.limits.max_observations = 0' testing/diagnostic-race.json > "$tmp/invalid-budget.json"
expect_failure invalid-budget "$tmp/invalid-budget.json"

jq '.required_context |= map(select(. != "creation-stack"))' \
    testing/diagnostic-race.json > "$tmp/missing-context.json"
expect_failure missing-context "$tmp/missing-context.json"

jq '.public_stdlib_api = true' testing/diagnostic-race.json > "$tmp/public-api.json"
expect_failure public-api "$tmp/public-api.json"

jq '.next_blocks = ["DIAG-TEST-001", "DIAG-CI-001"]' \
    testing/diagnostic-race.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

echo "diagnostic race tests: OK (status, algorithm, limits, context, privacy and sequencing negatives rejected)"
