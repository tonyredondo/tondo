#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

tmp_root="$(printenv TMPDIR || true)"
[[ -n "$tmp_root" ]] || tmp_root="/tmp"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-matrix-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

scripts/stdlib-matrix-check.sh

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib matrix tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[-1])' testing/stdlib-matrix.json > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_MATRIX="$tmp_dir/missing-owner.json" scripts/stdlib-matrix-check.sh

jq '(.owners[] | select(.id == "std.meta") | .stages["IMPL/HOST"].reason) = null' testing/stdlib-matrix.json > "$tmp_dir/missing-reason.json"
expect_failure missing-reason env TONDO_STDLIB_MATRIX="$tmp_dir/missing-reason.json" scripts/stdlib-matrix-check.sh

for stage in IMPL HOST MODEL TEST FUZZ; do
    jq --arg stage "$stage" '(.owners[] | select(.id == "std.core") | .cells) |= del(.[$stage])' \
        testing/stdlib-owner-evidence.json > "$tmp_dir/missing-cell.json"
    TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/missing-cell.json" \
        scripts/stdlib-matrix-generate.sh "$tmp_dir/from-missing-cell.json" >/dev/null
    jq -e --arg stage "$stage" '
        .owners[] | select(.id == "std.core")
        | .stages[if $stage == "IMPL" or $stage == "HOST" then "IMPL/HOST" else "MODEL/TEST/FUZZ" end].status == "gap"
    ' "$tmp_dir/from-missing-cell.json" >/dev/null
    TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/missing-cell.json" \
        TONDO_STDLIB_MATRIX="$tmp_dir/from-missing-cell.json" \
        scripts/stdlib-matrix-check.sh
done

jq '.rows |= map(select(.owner != "std.core"))' testing/stdlib-public-api.json > "$tmp_dir/empty-api.json"
TONDO_STDLIB_PUBLIC_API="$tmp_dir/empty-api.json" \
    scripts/stdlib-matrix-generate.sh "$tmp_dir/from-empty-api.json" >/dev/null
jq -e '.owners[] | select(.id == "std.core") | .stages["IMPL/HOST"].status == "partial"' \
    "$tmp_dir/from-empty-api.json" >/dev/null
TONDO_STDLIB_PUBLIC_API="$tmp_dir/empty-api.json" \
    TONDO_STDLIB_MATRIX="$tmp_dir/from-empty-api.json" \
    scripts/stdlib-matrix-check.sh

echo "stdlib normative matrix tests: OK"
