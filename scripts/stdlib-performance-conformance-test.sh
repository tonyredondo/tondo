#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-performance-test.XXXXXX")"

# Removing one owner must fail: the coordinator may defer a workload, but it
# may never silently omit an owner from the performance ledger.
jq 'del(.owners[0])' testing/stdlib-performance-conformance.json > "$tmp/missing-owner.json"
set +e
TONDO_STDLIB_PERF_CONFORMANCE_CONFIG="$tmp/missing-owner.json" \
    scripts/stdlib-performance-conformance.sh >/dev/null 2>&1
missing_owner_rc=$?
set -e
if [[ "$missing_owner_rc" -eq 0 ]]; then
    echo "stdlib performance conformance negative fixture unexpectedly passed" >&2
    exit 1
fi

# A captured owner must declare the exact dimensions currently measured by the
# probe; accepting a broader claim would make the report overstate evidence.
jq '(.owners[] | select(.state == "captured-partial") | .dimensions) = ["throughput"]' \
    testing/stdlib-performance-conformance.json > "$tmp/missing-dimension.json"
set +e
TONDO_STDLIB_PERF_CONFORMANCE_CONFIG="$tmp/missing-dimension.json" \
    scripts/stdlib-performance-conformance.sh >/dev/null 2>&1
missing_dimension_rc=$?
set -e
if [[ "$missing_dimension_rc" -eq 0 ]]; then
    echo "stdlib performance conformance dimension fixture unexpectedly passed" >&2
    exit 1
fi

echo "stdlib performance conformance tests: OK"
