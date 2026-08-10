#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-time-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.time owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.source.sha256 = "sha256:wrong"' testing/stdlib-time.json > "$tmp_dir/bad-hash.json"
expect_failure source-hash env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/bad-hash.json" scripts/stdlib-time-check.sh

jq '.capabilities.required = []' testing/stdlib-time.json > "$tmp_dir/missing-clock.json"
expect_failure required-clock env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/missing-clock.json" scripts/stdlib-time-check.sh

jq '.capabilities.forbidden += ["clock"]' testing/stdlib-time.json > "$tmp_dir/forbidden-clock.json"
expect_failure forbidden-clock env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/forbidden-clock.json" scripts/stdlib-time-check.sh

jq '.kind = "intrinsic"' testing/stdlib-time.json > "$tmp_dir/intrinsic-kind.json"
expect_failure capability-gated-kind env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/intrinsic-kind.json" scripts/stdlib-time-check.sh

echo "std.time owner tests: OK"
