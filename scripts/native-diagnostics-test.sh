#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${TMPDIR:-$root/.tmp}"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-diagnostics-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_DIAGNOSTICS_CONTRACT="$candidate" \
        scripts/native-diagnostics-check.sh >/dev/null 2>&1; then
        echo "native diagnostics tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-pending"' \
    testing/native-diagnostics.json > "$tmp/pending.json"
expect_failure pending-status "$tmp/pending.json"

jq '.backends = ["cranelift"]' \
    testing/native-diagnostics.json > "$tmp/missing-backend.json"
expect_failure missing-backend "$tmp/missing-backend.json"

jq '.envelope.format = "tondo-diagnostic-report/0"' \
    testing/native-diagnostics.json > "$tmp/wrong-envelope.json"
expect_failure wrong-envelope "$tmp/wrong-envelope.json"

jq '.corpus.race = ["race-conflict"]' \
    testing/native-diagnostics.json > "$tmp/missing-case.json"
expect_failure missing-case "$tmp/missing-case.json"

jq '.envelope.physical_data = "allowed"' \
    testing/native-diagnostics.json > "$tmp/physical-data.json"
expect_failure physical-data "$tmp/physical-data.json"

jq '.next_blocks = ["NATIVE-001"]' \
    testing/native-diagnostics.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.runtime.status_codes.clean = 4' \
    testing/native-diagnostics.json > "$tmp/wrong-status-code.json"
expect_failure wrong-status-code "$tmp/wrong-status-code.json"

echo "native diagnostics tests: OK (status, backend, envelope, corpus, privacy and sequencing negatives rejected)"
