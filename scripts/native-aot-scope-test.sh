#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-scope-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_AOT_SCOPE_CONTRACT="$candidate" scripts/native-aot-scope-check.sh \
        >/dev/null 2>&1; then
        echo "native AOT scope tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.product.primary = "tondo-vm-hosted"' \
    testing/native-aot-scope.json > "$tmp/non-aot-primary.json"
expect_failure non-aot-primary "$tmp/non-aot-primary.json"

jq '.product.jit = "candidate"' \
    testing/native-aot-scope.json > "$tmp/jit-included.json"
expect_failure jit-included "$tmp/jit-included.json"

jq '.memory.vm = "hybrid-arc-cycle-collector"' \
    testing/native-aot-scope.json > "$tmp/vm-native-memory.json"
expect_failure vm-native-memory "$tmp/vm-native-memory.json"

jq '.candidates.included = ["cranelift"]' \
    testing/native-aot-scope.json > "$tmp/candidate-drift.json"
expect_failure candidate-drift "$tmp/candidate-drift.json"

jq '.binary_size_metrics = ["code-buffer-bytes", "object-bytes", "debug-bytes"]' \
    testing/native-aot-scope.json > "$tmp/non-comparable-size.json"
expect_failure non-comparable-size "$tmp/non-comparable-size.json"

jq '.protocol.minimum_samples = 3' \
    testing/native-aot-scope.json > "$tmp/sample-drift.json"
expect_failure sample-drift "$tmp/sample-drift.json"

jq '.selection.selected_backend = "cranelift"' \
    testing/native-aot-scope.json > "$tmp/premature-selection.json"
expect_failure premature-selection "$tmp/premature-selection.json"

jq '.selection.n1_claim = true' \
    testing/native-aot-scope.json > "$tmp/premature-n1.json"
expect_failure premature-n1 "$tmp/premature-n1.json"

jq '.next_blocks = ["DEC-013"]' \
    testing/native-aot-scope.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

grep -Fq 'Raw Cranelift code-buffer lengths' docs/contracts/native-aot-scope.md
grep -Fq 'three fresh processes' docs/contracts/native-aot-scope.md
grep -Fq 'JIT is outside' docs/contracts/native-aot-scope.md

echo "native AOT scope tests: OK (product, memory, comparison and gate negatives rejected)"
