#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-memory-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_AOT_MEMORY_CONTRACT="$candidate" \
        scripts/native-aot-memory-check.sh >/dev/null 2>&1; then
        echo "native AOT memory tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-ready"' testing/native-aot-memory.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.protocol.minimum_sample_count = 9' testing/native-aot-memory.json > "$tmp/sample-count.json"
expect_failure sample-count "$tmp/sample-count.json"

jq '.dimensions = (.dimensions | map(select(. != "cycles_reclaimed")))' \
    testing/native-aot-memory.json > "$tmp/missing-cycle.json"
expect_failure missing-cycle "$tmp/missing-cycle.json"

jq '.oracle.counters = "language-semantics"' testing/native-aot-memory.json > "$tmp/semantic-counters.json"
expect_failure semantic-counters "$tmp/semantic-counters.json"

jq '.next_blocks = ["NATIVE-AOT-PERF-001"]' testing/native-aot-memory.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

scripts/native-aot-memory-check.sh >/dev/null
grep -Fq 'three warmups and then nine measured iterations' \
    docs/contracts/native-aot-memory.md
grep -Fq 'live bytes must return to zero' docs/contracts/native-aot-memory.md
grep -Fq 'ru_maxrss' docs/contracts/native-aot-memory.md

echo "native AOT memory tests: OK (protocol, counters, semantic and stale-frontier negatives)"
