#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-reflect-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.reflect owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.source.sha256 = "sha256:wrong"' testing/stdlib-reflect.json > "$tmp_dir/bad-hash.json"
expect_failure source-hash env TONDO_STDLIB_REFLECT_CONTRACT="$tmp_dir/bad-hash.json" scripts/stdlib-reflect-check.sh

jq '.kind = "runtime"' testing/stdlib-reflect.json > "$tmp_dir/runtime-kind.json"
expect_failure runtime-kind env TONDO_STDLIB_REFLECT_CONTRACT="$tmp_dir/runtime-kind.json" scripts/stdlib-reflect-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "runtime-value-reflection")]' \
    testing/stdlib-reflect.json > "$tmp_dir/value-reflection.json"
expect_failure value-reflection env TONDO_STDLIB_REFLECT_CONTRACT="$tmp_dir/value-reflection.json" scripts/stdlib-reflect-check.sh

cargo run -p tondo-cli --locked --quiet -- run tests/runtime/m11-std-reflect-public-001.to \
    > "$tmp_dir/public.stdout"
cmp -s "$tmp_dir/public.stdout" tests/runtime/m11-std-reflect-public-001.stdout || {
    echo "std.reflect owner tests: public output differs from its fixture" >&2
    exit 1
}

echo "std.reflect owner tests: OK (ordinary Tondo descriptor queries)"
