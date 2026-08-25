#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-runtime-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_RUNTIME_CONTRACT="$candidate" scripts/diagnostic-runtime-check.sh >/dev/null 2>&1; then
        echo "diagnostic runtime tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' testing/diagnostic-runtime.json > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.hooks_private = false' testing/diagnostic-runtime.json > "$tmp/public-hooks.json"
expect_failure public-hooks "$tmp/public-hooks.json"

jq '.limits.max_events = 0' testing/diagnostic-runtime.json > "$tmp/invalid-budget.json"
expect_failure invalid-budget "$tmp/invalid-budget.json"

jq '.next_blocks = ["RACE-001", "LEAK-001", "DUMP-001"]' testing/diagnostic-runtime.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.required_context |= map(select(. != "roots-retainers"))' \
    testing/diagnostic-runtime.json > "$tmp/missing-context.json"
expect_failure missing-context "$tmp/missing-context.json"

echo "diagnostic runtime tests: OK (status, privacy, budgets, sequencing and context negatives rejected)"
