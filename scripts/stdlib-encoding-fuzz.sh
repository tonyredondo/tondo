#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

nightly="${TONDO_ENCODING_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_ENCODING_FUZZ_RUNS:-128}"
seed="${TONDO_ENCODING_FUZZ_SEED:-4105}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
output_corpus="$target_dir/reliability/fuzz-corpus/stdlib-encoding"

[[ "$runs" =~ ^[1-9][0-9]*$ ]] || {
    echo "std.encoding fuzz: runs must be a positive integer" >&2
    exit 1
}
[[ "$seed" =~ ^[0-9]+$ ]] || {
    echo "std.encoding fuzz: seed must be an unsigned integer" >&2
    exit 1
}

export CARGO_TARGET_DIR="$target_dir"

rm -rf "$output_corpus"
mkdir -p "$output_corpus"
cargo "+$nightly" fuzz run stdlib_encoding "$output_corpus" \
    corpus/stdlib_encoding -- \
    "-runs=$runs" \
    "-seed=$seed" \
    -max_len=4096 \
    -timeout=10 \
    -rss_limit_mb=4096 \
    -print_final_stats=1

echo "std.encoding fuzz: OK ($runs runs, seed $seed, scalar/reference wire oracle)"
