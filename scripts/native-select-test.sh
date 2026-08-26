#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-select-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_SELECT_CONTRACT="$candidate" \
        scripts/native-select-check.sh >/dev/null 2>&1; then
        echo "native select tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/native-select.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.machine.max_arms = 0' testing/native-select.json > "$tmp/no-capacity.json"
expect_failure capacity "$tmp/no-capacity.json"

jq '.wakeup.polling_substitute = true' testing/native-select.json > "$tmp/polling.json"
expect_failure polling "$tmp/polling.json"

jq '.corpus.required_cases = []' testing/native-select.json > "$tmp/no-corpus.json"
expect_failure corpus "$tmp/no-corpus.json"

jq '.corpus.native_cases[0] = "broken"' testing/native-select.json > "$tmp/native-case.json"
expect_failure native-case "$tmp/native-case.json"

echo "native select tests: OK (contract rejects incomplete, unsafe and non-conforming variants)"
