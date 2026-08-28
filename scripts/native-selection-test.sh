#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-selection-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_SELECTION_CONTRACT="$candidate" scripts/native-selection-check.sh \
        >/dev/null 2>&1; then
        echo "native selection tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.selection.selected_backend = "llvm"' \
    testing/native-selection.json > "$tmp/wrong-backend.json"
expect_failure wrong-backend "$tmp/wrong-backend.json"

jq '.selection.n1_claim = true' \
    testing/native-selection.json > "$tmp/premature-n1.json"
expect_failure premature-n1 "$tmp/premature-n1.json"

jq '.selection.target_scope = ["aarch64-unknown-linux-gnu"]' \
    testing/native-selection.json > "$tmp/target-drift.json"
expect_failure target-drift "$tmp/target-drift.json"

jq '.selection.rationale = []' \
    testing/native-selection.json > "$tmp/missing-rationale.json"
expect_failure missing-rationale "$tmp/missing-rationale.json"

jq '.status = "ready-for-decision"' \
    testing/native-selection.json > "$tmp/stale-status.json"
expect_failure stale-status "$tmp/stale-status.json"

jq '.next_blocks = ["DEC-013"]' \
    testing/native-selection.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

scripts/native-selection-check.sh >/dev/null
grep -Fq 'Cranelift' docs/contracts/native-selection.md
grep -Fq 'Gate N1 remains open' docs/contracts/native-selection.md

echo "native selection tests: OK (decision identity, target, rationale and N1 guard rejected)"
