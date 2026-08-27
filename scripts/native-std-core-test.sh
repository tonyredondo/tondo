#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-std-core-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_STD_CORE_CONTRACT="$candidate" \
        scripts/native-std-core-check.sh >/dev/null 2>&1; then
        echo "native std.core tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.report_field = "legacy"' testing/native-std-core.json > "$tmp/legacy-report.json"
expect_failure legacy-report "$tmp/legacy-report.json"

jq '.cases = .cases[0:13]' testing/native-std-core.json > "$tmp/missing-case.json"
expect_failure missing-case "$tmp/missing-case.json"

jq '.next_blocks = ["NATIVE-001"]' testing/native-std-core.json > "$tmp/stale-frontier.json"
expect_failure stale-frontier "$tmp/stale-frontier.json"

jq '.invariants = []' testing/native-std-core.json > "$tmp/missing-invariants.json"
expect_failure missing-invariants "$tmp/missing-invariants.json"

echo "native std.core tests: OK (contract rejects stale, incomplete and underspecified boundaries)"
