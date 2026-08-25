#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-dump-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_DUMP_CONTRACT="$candidate" scripts/diagnostic-dump-check.sh >/dev/null 2>&1; then
        echo "diagnostic dump tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "pending"' testing/diagnostic-dump.json > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.schema = "tondo-dump/2"' testing/diagnostic-dump.json > "$tmp/wrong-schema.json"
expect_failure wrong-schema "$tmp/wrong-schema.json"

jq '.required_sections = ["header"]' testing/diagnostic-dump.json > "$tmp/missing-section.json"
expect_failure missing-section "$tmp/missing-section.json"

jq '.privacy.payloads = "included"' testing/diagnostic-dump.json > "$tmp/payloads.json"
expect_failure payloads "$tmp/payloads.json"

jq '.limits.max_dump_bytes = 0' testing/diagnostic-dump.json > "$tmp/invalid-limit.json"
expect_failure invalid-limit "$tmp/invalid-limit.json"

jq '.next_blocks = ["DIAG-TEST-001", "DIAG-CI-001"]' \
    testing/diagnostic-dump.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

echo "diagnostic dump tests: OK (status, schema, sections, privacy, limits and frontier negatives rejected)"
