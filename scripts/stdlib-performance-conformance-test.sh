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

# A captured owner must declare every dimension measured by the promoted
# report; accepting a narrower claim would make the report overstate evidence.
jq '(.owners[] | select(.state == "captured") | .dimensions) = ["throughput"]' \
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

# A not-applicable owner must carry a normative boundary, never an empty
# placeholder that could hide an unfinished benchmark.
jq '(.owners[] | select(.state == "not-applicable") | .reason) = ""' \
    testing/stdlib-performance-conformance.json > "$tmp/missing-na-reason.json"
set +e
TONDO_STDLIB_PERF_CONFORMANCE_CONFIG="$tmp/missing-na-reason.json" \
    scripts/stdlib-performance-conformance.sh >/dev/null 2>&1
missing_na_reason_rc=$?
set -e
if [[ "$missing_na_reason_rc" -eq 0 ]]; then
    echo "stdlib performance conformance not-applicable fixture unexpectedly passed" >&2
    exit 1
fi

# The report itself must carry every promoted dimension; a coordinator claim
# alone cannot promote an under-measured artifact.
jq '(.measurements[0].dimensions | del(.throughput))' \
    target/reliability/evidence/stdlib-performance-report.json > "$tmp/missing-report-dimension.json"
set +e
TONDO_STDLIB_PERF_REPORT="$tmp/missing-report-dimension.json" \
    scripts/stdlib-performance-conformance.sh >/dev/null 2>&1
missing_report_dimension_rc=$?
set -e
if [[ "$missing_report_dimension_rc" -eq 0 ]]; then
    echo "stdlib performance report dimension fixture unexpectedly passed" >&2
    exit 1
fi

echo "stdlib performance conformance tests: OK"
