#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

seconds="${TONDO_FUZZ_SECONDS:-180}"
nightly="${TONDO_FUZZ_NIGHTLY:-nightly-2026-07-28}"
for target in frontend protocols admission stdlib_codecs stdlib_owners; do
    case "$target" in
        frontend) seed=2001 ;;
        protocols) seed=2002 ;;
        admission) seed=2003 ;;
        stdlib_codecs) seed=2004 ;;
        stdlib_owners) seed=2022 ;;
    esac
    output_corpus="$root/target/reliability/fuzz-corpus/nightly/$target"
    rm -rf "$output_corpus"
    mkdir -p "$output_corpus"
    cargo "+$nightly" fuzz run "$target" "$output_corpus" "corpus/$target" -- \
        "-max_total_time=$seconds" \
        "-seed=$seed" \
        -timeout=10 \
        -rss_limit_mb=4096 \
        -print_final_stats=1
done
