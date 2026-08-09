#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

tmp_root="$(printenv TMPDIR || true)"
[[ -n "$tmp_root" ]] || tmp_root="/tmp"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-matrix-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

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

jq '.owners[0].stages.PERF.reason = null' testing/stdlib-matrix.json > "$tmp_dir/missing-reason.json"
expect_failure missing-reason env TONDO_STDLIB_MATRIX="$tmp_dir/missing-reason.json" scripts/stdlib-matrix-check.sh

echo "stdlib normative matrix tests: OK"
