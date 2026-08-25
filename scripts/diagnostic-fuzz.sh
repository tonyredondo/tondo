#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

mode="smoke"
if [[ "${1:-}" == "--campaign" ]]; then
    mode="campaign"
elif [[ "${1:-}" == "--smoke" || -z "${1:-}" ]]; then
    mode="smoke"
else
    echo "diagnostic fuzz: usage: $0 [--smoke|--campaign]" >&2
    exit 2
fi

nightly="${TONDO_DIAGNOSTIC_FUZZ_NIGHTLY:-nightly-2026-07-28}"
output="$root/target/reliability/fuzz-corpus/diagnostics/$mode"
mkdir -p "$output"

case "$mode" in
    smoke)
        runs="${TONDO_DIAGNOSTIC_FUZZ_RUNS:-128}"
        seed="${TONDO_DIAGNOSTIC_FUZZ_SEED:-4001}"
        cargo "+$nightly" fuzz run diagnostics \
            "$output" fuzz/corpus/diagnostics -- \
            "-runs=$runs" "-seed=$seed" -max_len=65536 -timeout=10 -rss_limit_mb=4096
        ;;
    campaign)
        seconds="${TONDO_DIAGNOSTIC_FUZZ_SECONDS:-180}"
        seed="${TONDO_DIAGNOSTIC_FUZZ_SEED:-5001}"
        cargo "+$nightly" fuzz run diagnostics \
            "$output" fuzz/corpus/diagnostics -- \
            "-max_total_time=$seconds" "-seed=$seed" -max_len=65536 -timeout=10 -rss_limit_mb=4096 \
            -print_final_stats=1
        ;;
esac

echo "diagnostic fuzz: OK mode=$mode target=diagnostics seed=$seed output=$output"
