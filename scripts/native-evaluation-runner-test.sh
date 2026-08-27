#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-runner-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_EVALUATION_RUNNER_CONTRACT="$candidate" \
        scripts/native-evaluation-runner-check.sh >/dev/null 2>&1; then
        echo "native evaluation runner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.adapter_format = "legacy-summary-only"' \
    testing/native-evaluation-runner.json > "$tmp/legacy-adapter.json"
expect_failure legacy-adapter "$tmp/legacy-adapter.json"

jq '.debug_format = "legacy"' \
    testing/native-evaluation-runner.json > "$tmp/legacy-debug.json"
expect_failure legacy-debug "$tmp/legacy-debug.json"

jq '.toolchain_policy.linker = "PATH"' \
    testing/native-evaluation-runner.json > "$tmp/ambient-linker.json"
expect_failure ambient-linker "$tmp/ambient-linker.json"

jq '.native_semantics = "vm-equivalent"' \
    testing/native-evaluation-runner.json > "$tmp/premature-equivalence.json"
expect_failure premature-equivalence "$tmp/premature-equivalence.json"

jq '.diagnostic_report_field = "legacy-diagnostics"' \
    testing/native-evaluation-runner.json > "$tmp/missing-diagnostics.json"
expect_failure missing-diagnostics "$tmp/missing-diagnostics.json"

jq '.diagnostic_phase = "NATIVE-001"' \
    testing/native-evaluation-runner.json > "$tmp/stale-diagnostics-phase.json"
expect_failure stale-diagnostics-phase "$tmp/stale-diagnostics-phase.json"

echo "native evaluation runner tests: OK (adapter, toolchain and equivalence boundaries rejected)"
