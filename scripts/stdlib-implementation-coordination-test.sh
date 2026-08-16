#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-implementation-coordination-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib implementation coordination tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[0])' testing/stdlib-implementation-coordination.json > "$tmp/missing-owner.json"
expect_failure missing-owner env \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/missing-owner.json" \
    scripts/stdlib-implementation-coordination-check.sh

jq '.owners[0].public_api.signatures[0].status = "open-gaps"' \
    testing/stdlib-implementation-coordination.json > "$tmp/signature-gap.json"
expect_failure signature-gap env \
    TONDO_STDLIB_IMPLEMENTATION_COORDINATION="$tmp/signature-gap.json" \
    scripts/stdlib-implementation-coordination-check.sh

echo "stdlib implementation coordination tests: OK"
