#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-native-publish.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native publish negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.negative_cases = ["unknown-fields"]' "$root/testing/native-publish.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives env TONDO_NATIVE_PUBLISH_CONTRACT="$tmp/incomplete-negatives.json" \
    "$root/scripts/native-publish-check.sh"

jq '.policy.commit = "write-in-place"' "$root/testing/native-publish.json" > "$tmp/in-place.json"
expect_failure in-place env TONDO_NATIVE_PUBLISH_CONTRACT="$tmp/in-place.json" \
    "$root/scripts/native-publish-check.sh"

jq '.consumer.mismatch = "execute-anyway"' "$root/testing/native-publish.json" > "$tmp/execute-anyway.json"
expect_failure execute-anyway env TONDO_NATIVE_PUBLISH_CONTRACT="$tmp/execute-anyway.json" \
    "$root/scripts/native-publish-check.sh"

jq '.phases = ["validate-records"]' "$root/testing/native-publish.json" > "$tmp/partial-phases.json"
expect_failure partial-phases env TONDO_NATIVE_PUBLISH_CONTRACT="$tmp/partial-phases.json" \
    "$root/scripts/native-publish-check.sh"

for marker in \
    'preserve-old-before-commit' \
    'commit-pair-atomically' \
    'directory-or-symlink-output' \
    'product-size-mismatch' \
    'receipt-over-limit' \
    'reject-before-exec'; do
    grep -Fq "$marker" "$root/docs/contracts/native-publish.md" "$root/testing/native-publish.json"
done

echo "native publish contract tests: OK"
