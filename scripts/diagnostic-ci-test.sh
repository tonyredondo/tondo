#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-ci-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_CI_CONTRACT="$candidate" scripts/diagnostic-ci-check.sh >/dev/null 2>&1; then
        echo "diagnostic CI tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.lanes[0].id = "unknown"' \
    testing/diagnostic-ci.json > "$tmp/unknown-lane.json"
expect_failure unknown-lane "$tmp/unknown-lane.json"

jq '.lanes[0].positive_tests = []' \
    testing/diagnostic-ci.json > "$tmp/missing-positive.json"
expect_failure missing-positive "$tmp/missing-positive.json"

jq '.fuzz.smoke.runs = 0' \
    testing/diagnostic-ci.json > "$tmp/zero-fuzz-runs.json"
expect_failure zero-fuzz-runs "$tmp/zero-fuzz-runs.json"

jq '.budgets.max_dump_bytes = 0' \
    testing/diagnostic-ci.json > "$tmp/zero-dump-budget.json"
expect_failure zero-dump-budget "$tmp/zero-dump-budget.json"

jq '.promotion.unsupported_is_failure = false' \
    testing/diagnostic-ci.json > "$tmp/unsupported-green.json"
expect_failure unsupported-green "$tmp/unsupported-green.json"

jq '.promotion.normal_baseline_unchanged = false' \
    testing/diagnostic-ci.json > "$tmp/baseline-mutation.json"
expect_failure baseline-mutation "$tmp/baseline-mutation.json"

jq '.next_blocks = ["DIAG-CI-001"]' \
    testing/diagnostic-ci.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.corpus.positive_root = "fuzz/corpus/diagnostics/missing"' \
    testing/diagnostic-ci.json > "$tmp/missing-corpus.json"
expect_failure missing-corpus "$tmp/missing-corpus.json"

grep -Fq 'fresh-process' docs/contracts/diagnostic-ci.md
grep -Fq 'unsupported' docs/contracts/diagnostic-ci.md
grep -Fq 'baseline normal' docs/contracts/diagnostic-ci.md

echo "diagnostic CI tests: OK (lane, corpus, fuzz, budget, unsupported, baseline and frontier negatives rejected)"
