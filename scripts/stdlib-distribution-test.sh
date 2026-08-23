#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-distribution-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib distribution negative case passed unexpectedly: $name" >&2
        exit 1
    fi
}

jq '.package_id = "wrong:std"' "$root/testing/stdlib-distribution.json" > "$tmp/wrong-package.json"
expect_failure wrong-package env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/wrong-package.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.archive.byte_identical = false' "$root/testing/stdlib-distribution.json" > "$tmp/non-reproducible.json"
expect_failure non-reproducible env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/non-reproducible.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.installation.workspace = "ambient-directory"' "$root/testing/stdlib-distribution.json" > "$tmp/ambient-workspace.json"
expect_failure ambient-workspace env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/ambient-workspace.json" \
    "$root/scripts/stdlib-distribution-check.sh"

jq '.negative_cases = ["binary-missing"]' "$root/testing/stdlib-distribution.json" > "$tmp/incomplete-negatives.json"
expect_failure incomplete-negatives env TONDO_STDLIB_DISTRIBUTION_CONTRACT="$tmp/incomplete-negatives.json" \
    "$root/scripts/stdlib-distribution-check.sh"

for marker in \
    'source_hashes' \
    'interface_hashes' \
    'unit_hashes' \
    'provider_hashes' \
    'manifest_hashes' \
    'capability_matrix_hash' \
    'manifest-and-file-hash-before-run' \
    'source-tree-required-after-install' \
    'uninstall_preserves_workspace'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-distribution.md" "$root/testing/stdlib-distribution.json" || {
        echo "stdlib distribution: missing test marker: $marker" >&2
        exit 1
    }
done

echo "stdlib distribution contract tests: OK"
