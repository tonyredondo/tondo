#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-lowering-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_LOWERING_CONTRACT="$candidate" \
        scripts/native-lowering-check.sh >/dev/null 2>&1; then
        echo "native lowering tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-ready"' testing/native-lowering.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.deferred.pending_state = 1' testing/native-lowering.json > "$tmp/eager.json"
expect_failure eager-body "$tmp/eager.json"

jq '.deferred.captures = "mutable-captures"' testing/native-lowering.json > "$tmp/mutable.json"
expect_failure mutable-capture "$tmp/mutable.json"

jq '.deferred.completion = "tondo_rt_task_complete_unchecked"' testing/native-lowering.json > "$tmp/unchecked.json"
expect_failure unchecked-completion "$tmp/unchecked.json"

jq '.slices = .slices[0:7]' testing/native-lowering.json > "$tmp/missing-slice.json"
expect_failure missing-slice "$tmp/missing-slice.json"

jq '.next_blocks = ["NATIVE-002"]' testing/native-lowering.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

scripts/native-lowering-check.sh >/dev/null
grep -Fq 'pending-before-join' docs/contracts/native-lowering.md
grep -Fq 'fail-closed' docs/contracts/native-lowering.md
grep -Fq 'tondo_rt_task_complete' crates/tondo-native-runtime/src/lib.rs
echo "native lowering tests: OK (coordination, deferred-state and fail-closed negatives rejected)"
