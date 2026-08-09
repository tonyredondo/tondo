#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-bytes-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.bytes owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.source.sha256 = "sha256:wrong"' testing/stdlib-bytes.json > "$tmp_dir/bad-hash.json"
expect_failure source-hash env TONDO_STDLIB_BYTES_CONTRACT="$tmp_dir/bad-hash.json" scripts/stdlib-bytes-check.sh

jq '.kind = "runtime"' testing/stdlib-bytes.json > "$tmp_dir/runtime-kind.json"
expect_failure intrinsic-kind env TONDO_STDLIB_BYTES_CONTRACT="$tmp_dir/runtime-kind.json" scripts/stdlib-bytes-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "ambient-host")]' \
    testing/stdlib-bytes.json > "$tmp_dir/ambient-host.json"
expect_failure ambient-host env TONDO_STDLIB_BYTES_CONTRACT="$tmp_dir/ambient-host.json" scripts/stdlib-bytes-check.sh

echo "std.bytes owner tests: OK"
