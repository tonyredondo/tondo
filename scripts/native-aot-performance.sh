#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$target_dir/reliability/evidence"
report="${TONDO_NATIVE_AOT_PERF_REPORT:-$evidence_dir/native-aot-performance.json}"
if [[ "$report" != /* ]]; then
    report="$root/$report"
fi
logs="$evidence_dir/native-aot-performance"
mkdir -p "$(dirname "$report")" "$logs"

die() {
    echo "native AOT performance: $*" >&2
    exit 1
}

[[ -x "$root/scripts/native-aot-performance-check.sh" ]] \
    || die "performance checker is not executable"
[[ -x "$root/scripts/native-aot-performance-test.sh" ]] \
    || die "performance mutation tests are not executable"

run_step() {
    local name="$1"
    shift
    echo "::group::$name"
    "$@" 2>&1 | tee "$logs/$name.log"
    echo "::endgroup::"
}

run_step contract-check scripts/native-aot-performance-check.sh
run_step contract-tests scripts/native-aot-performance-test.sh

if [[ "${TONDO_NATIVE_AOT_PERF_USE_EXISTING:-0}" == 1 ]]; then
    [[ -f "$report" ]] || die "requested existing performance report is missing: $report"
else
    run_step native-runner \
        env CARGO_TARGET_DIR="$target_dir" \
            TONDO_NATIVE_AOT_PERF_OUTPUT="$report" \
            scripts/native-evaluation-runner.sh
fi

run_step report-check \
    env TONDO_NATIVE_AOT_PERF_REPORT="$report" scripts/native-aot-performance-check.sh

echo "native AOT performance: PASS (report: ${report#"$root"/})"
