#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
seal_dir="${TONDO_STDLIB_S1A_SEAL_DIR:-$root/target/reliability/evidence/stdlib-s1a-seal}"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-s1a-seal-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib S1A seal negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

[[ -f "$seal_dir/seal.json" ]] || {
    echo "stdlib S1A seal tests: generated seal is missing" >&2
    exit 1
}
[[ -f "$seal_dir/bundle/tondo-stdlib-s1a/metadata/manifest.json" ]] || {
    echo "stdlib S1A seal tests: generated bundle is missing" >&2
    exit 1
}

jq '.invariants.matrix.open_rows = 1' "$root/testing/stdlib-s1a-seal.json" > "$tmp/open-cell.json"
expect_failure matrix-open-cell env TONDO_STDLIB_S1A_SEAL_CONTRACT="$tmp/open-cell.json" \
    "$root/scripts/stdlib-s1a-seal-check.sh"

mkdir -p "$tmp/claims/bundle/tondo-stdlib-s1a/metadata"
jq '.g5_claim = true' "$seal_dir/seal.json" > "$tmp/claims/seal.json"
cp -- "$seal_dir/bundle/tondo-stdlib-s1a/metadata/manifest.json" \
    "$tmp/claims/bundle/tondo-stdlib-s1a/metadata/manifest.json"
expect_failure g5-claim env TONDO_STDLIB_S1A_SEAL_DIR="$tmp/claims" \
    "$root/scripts/stdlib-s1a-seal-check.sh"

mkdir -p "$tmp/payload/bundle/tondo-stdlib-s1a/metadata"
cp -- "$seal_dir/seal.json" "$tmp/payload/seal.json"
jq '.files[0].sha256 = ("0" * 64)' \
    "$seal_dir/bundle/tondo-stdlib-s1a/metadata/manifest.json" \
    > "$tmp/payload/bundle/tondo-stdlib-s1a/metadata/manifest.json"
expect_failure bundle-manifest-mismatch env TONDO_STDLIB_S1A_SEAL_DIR="$tmp/payload" \
    "$root/scripts/stdlib-s1a-seal-check.sh"

echo "stdlib S1A seal tests: OK (matrix, claim and payload negatives rejected)"
