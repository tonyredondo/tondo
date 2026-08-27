#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-arc-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_ARC_CONTRACT="$candidate" \
        scripts/native-arc-check.sh >/dev/null 2>&1; then
        echo "native ARC tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/native-arc.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.ownership.shared = "plain-u32"' testing/native-arc.json > "$tmp/non-atomic.json"
expect_failure shared-count "$tmp/non-atomic.json"

jq '.cycle_collection.weak_refs = "strong-edge"' testing/native-arc.json > "$tmp/strong-weak.json"
expect_failure weak-edge "$tmp/strong-weak.json"

jq '.next_blocks = ["ARC-001"]' testing/native-arc.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

jq '.corpus.arc_001_cases = .corpus.arc_001_cases[0:6]' testing/native-arc.json > "$tmp/missing-case.json"
expect_failure missing-case "$tmp/missing-case.json"

jq '.status = "closed" | .implemented_blocks = ["ARC-001", "ARC-002"] | .pending_blocks = [] | .next_blocks = ["NATIVE-STD-CORE-001"] | .corpus.arc_002_cases = .corpus.arc_002_cases[0:2]' testing/native-arc.json > "$tmp/missing-cycle-case.json"
expect_failure missing-cycle-case "$tmp/missing-cycle-case.json"

scripts/native-arc-check.sh >/dev/null
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" \
    cargo test -p tondo-native-runtime --locked
echo "native ARC tests: OK (ownership, terminal cleanup and fail-closed contract negatives)"
