#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

nightly="${TONDO_GROUP_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_GROUP_FUZZ_RUNS:-128}"
seed="${TONDO_GROUP_FUZZ_SEED:-4101}"
output_corpus="$root/target/reliability/fuzz-corpus/stdlib-async-group"

[[ "$runs" =~ ^[1-9][0-9]*$ ]] || {
    echo "std.async.Group fuzz: runs must be a positive integer" >&2
    exit 1
}
[[ "$seed" =~ ^[0-9]+$ ]] || {
    echo "std.async.Group fuzz: seed must be an unsigned integer" >&2
    exit 1
}

rm -rf "$output_corpus"
mkdir -p "$output_corpus"
cargo "+$nightly" fuzz run stdlib_async_group "$output_corpus" \
    corpus/stdlib_async_group -- \
    "-runs=$runs" \
    "-seed=$seed" \
    -max_len=4096 \
    -timeout=10 \
    -rss_limit_mb=4096 \
    -print_final_stats=1

echo "std.async.Group fuzz: OK ($runs runs, seed $seed, bounded model oracle)"
