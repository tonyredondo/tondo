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

jq '.capabilities.optional = []' testing/stdlib-time.json > "$tmp_dir/missing-clock.json"
expect_failure optional-clock env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/missing-clock.json" scripts/stdlib-time-check.sh

jq '.capabilities.required = ["clock"]' testing/stdlib-time.json > "$tmp_dir/module-clock.json"
expect_failure module-clock env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/module-clock.json" scripts/stdlib-time-check.sh

jq '.capabilities.scope = "module"' testing/stdlib-time.json > "$tmp_dir/module-scope.json"
expect_failure module-scope env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/module-scope.json" scripts/stdlib-time-check.sh

for operation in $(jq -r '.capabilities.operations.clock | keys[]' testing/stdlib-time.json); do
    jq --argjson operation "$operation" 'del(.capabilities.operations.clock[$operation])' \
        testing/stdlib-time.json > "$tmp_dir/missing-operation.json"
    expect_failure "missing-operation-$operation" env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/missing-operation.json" scripts/stdlib-time-check.sh
done

for operation in $(jq -r '.capabilities.pure | keys[]' testing/stdlib-time.json); do
    jq --argjson operation "$operation" 'del(.capabilities.pure[$operation])' \
        testing/stdlib-time.json > "$tmp_dir/missing-pure.json"
    expect_failure "missing-pure-$operation" env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/missing-pure.json" scripts/stdlib-time-check.sh
done

jq '.capabilities.forbidden += ["clock"]' testing/stdlib-time.json > "$tmp_dir/forbidden-clock.json"
expect_failure forbidden-clock env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/forbidden-clock.json" scripts/stdlib-time-check.sh

jq '.kind = "intrinsic"' testing/stdlib-time.json > "$tmp_dir/intrinsic-kind.json"
expect_failure capability-gated-kind env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/intrinsic-kind.json" scripts/stdlib-time-check.sh

jq '.capabilities.pure_extensions = []' testing/stdlib-time.json > "$tmp_dir/missing-civil-core.json"
expect_failure missing-civil-core env TONDO_STDLIB_TIME_CONTRACT="$tmp_dir/missing-civil-core.json" scripts/stdlib-time-check.sh
echo "std.time owner tests: OK"
