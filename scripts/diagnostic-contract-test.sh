#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-diagnostic-contract-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_DIAGNOSTIC_CONTRACT="$candidate" scripts/diagnostic-contract-check.sh >/dev/null 2>&1; then
        echo "diagnostic contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.profiles[0].id = "unknown"' \
    testing/diagnostic-tooling.json > "$tmp/unknown-profile.json"
expect_failure unknown-profile "$tmp/unknown-profile.json"

jq '.report.required_fields |= map(select(. != "attempt_id"))' \
    testing/diagnostic-tooling.json > "$tmp/missing-identity.json"
expect_failure missing-identity "$tmp/missing-identity.json"

jq '.limits.max_events = 0' \
    testing/diagnostic-tooling.json > "$tmp/invalid-limit.json"
expect_failure invalid-limit "$tmp/invalid-limit.json"

jq '.cli.stdlib_api_added = true' \
    testing/diagnostic-tooling.json > "$tmp/parallel-stdlib-api.json"
expect_failure parallel-stdlib-api "$tmp/parallel-stdlib-api.json"

jq '.compilation_diagnostics_contract = "docs/contracts/other.json"' \
    testing/diagnostic-tooling.json > "$tmp/compilation-schema-drift.json"
expect_failure compilation-schema-drift "$tmp/compilation-schema-drift.json"

jq '.privacy.network_upload = true' \
    testing/diagnostic-tooling.json > "$tmp/network-upload.json"
expect_failure network-upload "$tmp/network-upload.json"

echo "diagnostic contract tests: OK (profile, identity, limits, boundaries and privacy negatives rejected)"
