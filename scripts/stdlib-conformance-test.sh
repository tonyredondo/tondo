#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-conformance-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

scripts/stdlib-conformance-check.sh --plan
[[ $# == 0 || ($# == 1 && "$1" == --plan) ]] || { echo "usage: stdlib-conformance-test.sh [--plan]" >&2; exit 1; }

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib conformance: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners[0].status = "partial"' testing/stdlib-conformance.json >"$tmp_dir/partial-owner.json"
expect_failure partial-owner env TONDO_STDLIB_CONFORMANCE_CONTRACT="$tmp_dir/partial-owner.json" scripts/stdlib-conformance-check.sh --plan

jq '.status = "promoted" | .owners[].status = "verified"' testing/stdlib-conformance.json > "$tmp_dir/invented-promotion.json"
expect_failure invented-promotion env TONDO_STDLIB_CONFORMANCE_CONTRACT="$tmp_dir/invented-promotion.json" scripts/stdlib-conformance-check.sh --plan

if [[ "${1:-}" == --plan ]]; then
    echo "stdlib conformance case-plan tests: OK; execution evidence was not tested"
    exit 0
fi
scripts/stdlib-conformance-check.sh
evidence="${TONDO_STDLIB_CONFORMANCE_EVIDENCE:-${CARGO_TARGET_DIR:-target}/reliability/evidence/stdlib-conformance.json}"

jq '.full_suite.cases -= 1' "$evidence" >"$tmp_dir/bad-evidence.json"
expect_failure bad-evidence env TONDO_STDLIB_CONFORMANCE_EVIDENCE="$tmp_dir/bad-evidence.json" scripts/stdlib-conformance-check.sh

jq '.owners[0].rows.total = 0' "$evidence" >"$tmp_dir/missing-row.json"
expect_failure missing-row env TONDO_STDLIB_CONFORMANCE_EVIDENCE="$tmp_dir/missing-row.json" scripts/stdlib-conformance-check.sh

for mutation in \
    '.tree_sha256 = "stale"' \
    '.commands |= map(select(.id != "owner-core"))' \
    '.commands[0].log_sha256 = ("0" * 64)' \
    '.cases = .cases[1:]' \
    '.full_suite.result_sha256 = ("0" * 64)' \
    '.owners[0].case_ids = []' \
    '.promotion = "promoted"'; do
    jq "$mutation" "$evidence" > "$tmp_dir/invalid-observation.json"
    expect_failure invalid-observation env \
        TONDO_STDLIB_CONFORMANCE_EVIDENCE="$tmp_dir/invalid-observation.json" \
        scripts/stdlib-conformance-check.sh
done

echo "stdlib conformance tests: OK"
