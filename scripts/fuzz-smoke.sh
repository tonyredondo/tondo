#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

runs="${TONDO_FUZZ_RUNS:-128}"
nightly="${TONDO_FUZZ_NIGHTLY:-nightly-2026-07-28}"
for target in frontend protocols admission stdlib_codecs stdlib_owners; do
    case "$target" in
        frontend) seed=1001 ;;
        protocols) seed=1002 ;;
        admission) seed=1003 ;;
        stdlib_codecs) seed=1004 ;;
        stdlib_owners) seed=1022 ;;
    esac
    output_corpus="$root/target/reliability/fuzz-corpus/smoke/$target"
    rm -rf "$output_corpus"
    mkdir -p "$output_corpus"
    cargo "+$nightly" fuzz run "$target" "$output_corpus" "corpus/$target" -- \
        "-runs=$runs" \
        "-seed=$seed" \
        -timeout=10 \
        -rss_limit_mb=4096
done
