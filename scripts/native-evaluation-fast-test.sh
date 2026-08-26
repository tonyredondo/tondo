#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-fast-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_EVALUATION_FAST_CONTRACT="$candidate" scripts/native-evaluation-fast-check.sh \
        >/dev/null 2>&1; then
        echo "native evaluation fast tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.selection.selected_backend = "cranelift"' \
    testing/native-evaluation-fast.json > "$tmp/selected-too-early.json"
expect_failure selected-too-early "$tmp/selected-too-early.json"

jq '.protocol.minimum_sample_count = 27' \
    testing/native-evaluation-fast.json > "$tmp/wrong-sample-count.json"
expect_failure wrong-sample-count "$tmp/wrong-sample-count.json"

jq '.corpus[0].sha256 = ("0" * 64)' \
    testing/native-evaluation-fast.json > "$tmp/corpus-hash-mismatch.json"
expect_failure corpus-hash-mismatch "$tmp/corpus-hash-mismatch.json"

jq '.oracle["runtime-equivalence"] = "available"' \
    testing/native-evaluation-fast.json > "$tmp/premature-semantics.json"
expect_failure premature-semantics "$tmp/premature-semantics.json"

jq '.adapter.unsupported_policy = "ignore"' \
    testing/native-evaluation-fast.json > "$tmp/non-fail-closed-adapter.json"
expect_failure non-fail-closed-adapter "$tmp/non-fail-closed-adapter.json"

jq '.candidates[0].id = "unknown"' \
    testing/native-evaluation-fast.json > "$tmp/unknown-candidate.json"
expect_failure unknown-candidate "$tmp/unknown-candidate.json"

echo "native evaluation fast tests: OK (pending-selection and fast-lane invariants rejected)"
