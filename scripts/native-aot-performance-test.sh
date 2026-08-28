#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${TONDO_NATIVE_AOT_PERF_TMPDIR:-$root/.tmp}"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1" candidate="$2"
    if TONDO_NATIVE_AOT_PERF_CONTRACT="$candidate" scripts/native-aot-performance-check.sh >/dev/null 2>&1; then
        echo "native AOT performance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

for mutation in \
    '(.status = "evaluation-ready")' \
    '(.protocol.measurement_repetitions = 8)' \
    '(.workloads[1].id = .workloads[0].id)' \
    '(.input.candidates = ["cranelift"])' \
    '(.input.jit = "included")' \
    '(.required_global_workloads = .required_global_workloads[1:])' \
    '(.input.product = "object-only")' \
    '(.selection.selected_backend = "llvm")' \
    '(.dimensions |= map(select(.id != "pause_time")))' \
    '(.dimensions |= map(select(.id != "build_end_to_end")))' \
    '(.next_blocks = ["N1"])' \
    '(.invariants = .invariants[1:])'; do
    name="mutation-${#mutation}"
    jq "$mutation" testing/native-aot-performance.json > "$tmp/$name.json"
    expect_failure "$name" "$tmp/$name.json"
done

if TONDO_NATIVE_AOT_PERF_REPORT="$tmp/missing-report.json" \
    scripts/native-aot-performance-check.sh >/dev/null 2>&1; then
    echo "native AOT performance tests: missing-report unexpectedly passed" >&2
    exit 1
fi

scripts/native-aot-performance-check.sh >/dev/null
grep -Fq 'complete linked AOT products' docs/contracts/native-aot-performance.md
grep -Fq 'build_end_to_end_ns' docs/contracts/native-aot-performance.md
grep -Fq 'human-decision-required' docs/contracts/native-aot-performance.md
grep -Fq 'NATIVE-AOT-MEM-001' docs/contracts/native-aot-performance.md

echo "native AOT performance tests: OK (12 contract mutations and missing-report rejection)"
