#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-lowering-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_AOT_LOWERING_CONTRACT="$candidate" \
        scripts/native-aot-lowering-check.sh >/dev/null 2>&1; then
        echo "native AOT lowering tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-ready"' testing/native-aot-lowering.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.corpus.storage_cases = .corpus.storage_cases[0:6]' \
    testing/native-aot-lowering.json > "$tmp/missing-storage.json"
expect_failure missing-storage "$tmp/missing-storage.json"

jq '.admitted.calls = ["direct"]' testing/native-aot-lowering.json > "$tmp/no-indirect.json"
expect_failure missing-indirect "$tmp/no-indirect.json"

jq '.input.same_input = false' testing/native-aot-lowering.json > "$tmp/different-mir.json"
expect_failure different-mir "$tmp/different-mir.json"

jq '.inventory.trap_policy = "ignore"' testing/native-aot-lowering.json > "$tmp/no-traps.json"
expect_failure trap-policy "$tmp/no-traps.json"

jq '.next_blocks = ["NATIVE-002"]' testing/native-aot-lowering.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

scripts/native-aot-lowering-check.sh >/dev/null
grep -Fq 'one immutable' docs/contracts/native-aot-lowering.md
grep -Fq 'aggregate-set' docs/contracts/native-aot-lowering.md
grep -Fq 'explicit unsupported function' docs/contracts/native-aot-lowering.md

echo "native AOT lowering tests: OK (contract drift and incomplete admitted surface rejected)"
