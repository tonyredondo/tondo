#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-env-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.env owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.source.sha256 = "sha256:wrong"' testing/stdlib-env.json > "$tmp_dir/bad-hash.json"
expect_failure source-hash env TONDO_STDLIB_ENV_CONTRACT="$tmp_dir/bad-hash.json" scripts/stdlib-env-check.sh

jq '.capabilities.required = []' testing/stdlib-env.json > "$tmp_dir/missing-environment.json"
expect_failure required-environment env TONDO_STDLIB_ENV_CONTRACT="$tmp_dir/missing-environment.json" scripts/stdlib-env-check.sh

jq '.capabilities.forbidden += ["environment"]' testing/stdlib-env.json > "$tmp_dir/forbidden-environment.json"
expect_failure forbidden-environment env TONDO_STDLIB_ENV_CONTRACT="$tmp_dir/forbidden-environment.json" scripts/stdlib-env-check.sh

jq '.kind = "intrinsic"' testing/stdlib-env.json > "$tmp_dir/intrinsic-kind.json"
expect_failure capability-gated-kind env TONDO_STDLIB_ENV_CONTRACT="$tmp_dir/intrinsic-kind.json" scripts/stdlib-env-check.sh

jq '.invariants = .invariants[0:8]' testing/stdlib-env.json > "$tmp_dir/incomplete-invariants.json"
expect_failure invariant-count env TONDO_STDLIB_ENV_CONTRACT="$tmp_dir/incomplete-invariants.json" scripts/stdlib-env-check.sh

echo "std.env owner tests: OK"
