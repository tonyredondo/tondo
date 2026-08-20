#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-fuzz-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib fuzz: $name unexpectedly passed" >&2
        exit 1
    fi
}

scripts/stdlib-fuzz-check.sh >/dev/null

jq '.owners = .owners[1:]' testing/stdlib-fuzz.json > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/missing-owner.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].corpus = "fuzz/corpus/stdlib_owners/missing/seed"' testing/stdlib-fuzz.json > "$tmp_dir/missing-corpus.json"
expect_failure missing-corpus env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/missing-corpus.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].route = "std.unknown"' testing/stdlib-fuzz.json > "$tmp_dir/unknown-route.json"
expect_failure unknown-route env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/unknown-route.json" scripts/stdlib-fuzz-check.sh

echo "stdlib fuzz tests: OK"
