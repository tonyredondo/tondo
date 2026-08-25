#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_TEST_CONTRACT="$candidate" "$root/scripts/diagnostic-test-check.sh" >/dev/null 2>&1; then
        echo "diagnostic test: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' "$root/testing/diagnostic-test.json" > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.worker.process_format = "tondo-test-worker-process/1"' \
    "$root/testing/diagnostic-test.json" > "$tmp/stale-worker.json"
expect_failure stale-worker-protocol "$tmp/stale-worker.json"

jq '.worker.fresh_process_per_attempt = false' \
    "$root/testing/diagnostic-test.json" > "$tmp/reused-worker.json"
expect_failure reused-worker "$tmp/reused-worker.json"

jq '.report.limitations_required = false' \
    "$root/testing/diagnostic-test.json" > "$tmp/optional-limitations.json"
expect_failure optional-limitations "$tmp/optional-limitations.json"

jq '.next_blocks = ["DIAG-TEST-001", "DIAG-CI-001"]' \
    "$root/testing/diagnostic-test.json" > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.negative_cases = ["unknown-profile"]' \
    "$root/testing/diagnostic-test.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives "$tmp/incomplete-negatives.json"

for marker in \
    'retry_state_isolated' \
    'shard_identity_preserved' \
    'artifacts_content_addressed' \
    'unsupported' \
    'tondo.diagnostics'; do
    grep -Fq "$marker" "$root/testing/diagnostic-test.json" "$root/docs/contracts/diagnostic-test.md" "$root/crates/tondo-cli/src/main.rs"
done

echo "diagnostic test: OK (protocol, isolation, limitations, frontier and artifact negatives rejected)"
