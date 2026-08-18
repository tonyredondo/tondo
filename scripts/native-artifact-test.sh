#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-native-artifact.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native artifact negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.node_kinds = ["object"]' "$root/testing/native-artifact.json" > "$tmp/missing-kind.json"
expect_failure missing-kind env TONDO_NATIVE_ARTIFACT_CONTRACT="$tmp/missing-kind.json" \
    "$root/scripts/native-artifact-check.sh"

jq '.graph.reproducible = false' "$root/testing/native-artifact.json" > "$tmp/not-reproducible.json"
expect_failure not-reproducible env TONDO_NATIVE_ARTIFACT_CONTRACT="$tmp/not-reproducible.json" \
    "$root/scripts/native-artifact-check.sh"

jq '.negative_cases = ["unknown-fields"]' "$root/testing/native-artifact.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives env TONDO_NATIVE_ARTIFACT_CONTRACT="$tmp/incomplete-negatives.json" \
    "$root/scripts/native-artifact-check.sh"

for marker in \
    'tondo-native-artifact-draft' \
    'target_descriptor_hash' \
    'source_artifact_hash' \
    'artifact_hash' \
    'NATIVE-LINK-PLAN-001' \
    'NATIVE-PUBLISH-SPEC-001'; do
    grep -Fq "$marker" "$root/docs/contracts/native-artifact.md"
done

echo "native artifact contract tests: OK"
