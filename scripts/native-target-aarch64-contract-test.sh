#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-native-target-aarch64.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "native target ARM64 negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

scripts/native-target-aarch64-check.sh

jq '.targets[0].triple = "x86_64-unknown-linux-gnu"' \
    "$root/testing/native-target-aarch64.json" > "$tmp/wrong-triple.json"
expect_failure wrong-triple env TONDO_NATIVE_TARGET_ARM64_REGISTRY="$tmp/wrong-triple.json" \
    "$root/scripts/native-target-aarch64-check.sh"

jq '.targets[0].backends = ["llvm"]' \
    "$root/testing/native-target-aarch64.json" > "$tmp/backend-drift.json"
expect_failure backend-drift env TONDO_NATIVE_TARGET_ARM64_REGISTRY="$tmp/backend-drift.json" \
    "$root/scripts/native-target-aarch64-check.sh"

echo "native target ARM64 contract tests: OK"
