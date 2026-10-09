#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root/fuzz"
nightly="${TONDO_LOG_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_LOG_FUZZ_RUNS:-128}"
seed="${TONDO_LOG_FUZZ_SEED:-4113}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$target_dir" == /* ]] || target_dir="$root/$target_dir"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo 'std.log fuzz: runs must be positive' >&2; exit 1; }
[[ "$seed" =~ ^[0-9]+$ ]] || { echo 'std.log fuzz: seed must be unsigned' >&2; exit 1; }
export CARGO_TARGET_DIR="$target_dir"
mkdir -p "$target_dir/reliability/fuzz-corpus"
output_corpus="$(mktemp -d "$target_dir/reliability/fuzz-corpus/stdlib-log.XXXXXX")"
trap 'rm -rf "$output_corpus"' EXIT
cargo "+$nightly" fuzz run --no-default-features stdlib_log "$output_corpus" \
    corpus/stdlib_log -- "-runs=$runs" "-seed=$seed" \
    -max_len=4096 -timeout=10 -rss_limit_mb=4096 -print_final_stats=1
echo "std.log fuzz: OK ($runs runs; seed $seed; bounded models and shared kernel comparisons)"
