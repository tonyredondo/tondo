#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-native-link-plan.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native link plan negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.negative_cases = ["unknown-fields"]' "$root/testing/native-link-plan.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives env TONDO_NATIVE_LINK_PLAN_CONTRACT="$tmp/incomplete-negatives.json" \
    "$root/scripts/native-link-plan-check.sh"

jq '.bindings.driver = "ambient-driver"' "$root/testing/native-link-plan.json" > "$tmp/ambient-driver.json"
expect_failure ambient-driver env TONDO_NATIVE_LINK_PLAN_CONTRACT="$tmp/ambient-driver.json" \
    "$root/scripts/native-link-plan-check.sh"

jq '.limits.positive = false' "$root/testing/native-link-plan.json" > "$tmp/unbounded.json"
expect_failure unbounded env TONDO_NATIVE_LINK_PLAN_CONTRACT="$tmp/unbounded.json" \
    "$root/scripts/native-link-plan-check.sh"

for marker in \
    'tondo-native-link-plan-draft' \
    'artifact_target_descriptor_hash' \
    'max_output_bytes' \
    'ordered arguments' \
    'NATIVE-PUBLISH-SPEC-001'; do
    grep -Fq "$marker" "$root/docs/contracts/native-link-plan.md"
done

echo "native link plan contract tests: OK"
