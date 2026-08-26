#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-lowering-debug-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_LOWERING_DEBUG_CONTRACT="$candidate" \
        scripts/native-lowering-debug-check.sh >/dev/null 2>&1; then
        echo "native lowering debug tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-ready"' testing/native-lowering-debug.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.debug_format = "legacy"' testing/native-lowering-debug.json > "$tmp/legacy.json"
expect_failure format "$tmp/legacy.json"

jq '.invariants.logical_paths_only = false' testing/native-lowering-debug.json > "$tmp/physical-paths.json"
expect_failure physical-paths "$tmp/physical-paths.json"

jq '.invariants.execution_kinds = ["task"]' testing/native-lowering-debug.json > "$tmp/task-only.json"
expect_failure execution-kinds "$tmp/task-only.json"

jq '.next_blocks = ["NATIVE-LOWER-DEBUG-001"]' testing/native-lowering-debug.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

echo "native lowering debug tests: OK (format, privacy, identity, execution and frontier negatives rejected)"
