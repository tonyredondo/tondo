#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

nightly="${TONDO_YAML_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_YAML_FUZZ_RUNS:-128}"
seed="${TONDO_YAML_FUZZ_SEED:-4107}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
output_corpus="$target_dir/reliability/fuzz-corpus/stdlib-yaml"

[[ "$runs" =~ ^[1-9][0-9]*$ ]] || {
    echo "std.yaml fuzz: runs must be a positive integer" >&2
    exit 1
}
[[ "$seed" =~ ^[0-9]+$ ]] || {
    echo "std.yaml fuzz: seed must be an unsigned integer" >&2
    exit 1
}

export CARGO_TARGET_DIR="$target_dir"

rm -rf "$output_corpus"
mkdir -p "$output_corpus"
cargo "+$nightly" fuzz run stdlib_yaml "$output_corpus" \
    corpus/stdlib_yaml -- \
    "-runs=$runs" \
    "-seed=$seed" \
    -max_len=4096 \
    -timeout=10 \
    -rss_limit_mb=4096 \
    -print_final_stats=1

echo "std.yaml fuzz: OK ($runs runs, seed $seed, independent canonical model and hosted parser boundary)"
