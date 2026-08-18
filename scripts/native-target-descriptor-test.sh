#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-native-target-descriptor.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native target descriptor negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.object_formats = ["elf"]' "$root/testing/native-target-descriptor.json" > "$tmp/missing-object-format.json"
expect_failure missing-object-format env TONDO_NATIVE_TARGET_CONTRACT="$tmp/missing-object-format.json" \
    "$root/scripts/native-target-descriptor-check.sh"

jq '.selection.path_lookup = "allowed"' "$root/testing/native-target-descriptor.json" > "$tmp/path-lookup.json"
expect_failure path-lookup env TONDO_NATIVE_TARGET_CONTRACT="$tmp/path-lookup.json" \
    "$root/scripts/native-target-descriptor-check.sh"

for marker in \
    'sha256(canonical-descriptor-bytes)' \
    'PATH lookup' \
    'NATIVE-ARTIFACT-001' \
    'NATIVE-LINK-PLAN-001'; do
    grep -Fq "$marker" "$root/docs/contracts/native-target-descriptor.md"
done

echo "native target descriptor contract tests: OK"
