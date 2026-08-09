#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-meta-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.meta owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.package.source_hash = "sha256:wrong"' testing/stdlib-meta.json > "$tmp_dir/bad-contract.json"
expect_failure source-hash env TONDO_STDLIB_META_CONTRACT="$tmp_dir/bad-contract.json" scripts/stdlib-meta-check.sh

jq '.owners[0].cells.HOST.status = "verified" | .owners[0].cells.HOST.reason = null' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/bad-evidence.json"
expect_failure host-boundary env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/bad-evidence.json" scripts/stdlib-owner-evidence-check.sh

jq '.owners[0].cells.TEST.refs[0] = "missing/std-meta-test-reference"' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/missing-reference.json"
expect_failure missing-reference env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/missing-reference.json" scripts/stdlib-owner-evidence-check.sh

echo "std.meta owner tests: OK"
