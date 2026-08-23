#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-conformance-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib conformance: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners[0].status = "partial"' testing/stdlib-conformance.json >"$tmp_dir/partial-owner.json"
expect_failure partial-owner env TONDO_STDLIB_CONFORMANCE_CONTRACT="$tmp_dir/partial-owner.json" scripts/stdlib-conformance.sh

jq '.full_suite.cases = 205' target/reliability/evidence/stdlib-conformance.json >"$tmp_dir/bad-evidence.json"
expect_failure bad-evidence env TONDO_STDLIB_CONFORMANCE_EVIDENCE="$tmp_dir/bad-evidence.json" scripts/stdlib-conformance-check.sh

jq '.owners[0].rows.total = 0' target/reliability/evidence/stdlib-conformance.json >"$tmp_dir/missing-row.json"
expect_failure missing-row env TONDO_STDLIB_CONFORMANCE_EVIDENCE="$tmp_dir/missing-row.json" scripts/stdlib-conformance-check.sh

echo "stdlib conformance tests: OK"
