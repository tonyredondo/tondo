#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root/fuzz"

nightly="${TONDO_SYNC_COLLECTION_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_SYNC_COLLECTION_FUZZ_RUNS:-128}"
seed="${TONDO_SYNC_COLLECTION_FUZZ_SEED:-4103}"
output_corpus="$root/target/reliability/fuzz-corpus/stdlib-sync-collections"

[[ "$runs" =~ ^[1-9][0-9]*$ ]] || {
    echo "std.sync collection fuzz: runs must be a positive integer" >&2
    exit 1
}
[[ "$seed" =~ ^[0-9]+$ ]] || {
    echo "std.sync collection fuzz: seed must be an unsigned integer" >&2
    exit 1
}

rm -rf "$output_corpus"
mkdir -p "$output_corpus"
cargo "+$nightly" fuzz run stdlib_sync_collections "$output_corpus" \
    corpus/stdlib_sync_collections -- \
    "-runs=$runs" \
    "-seed=$seed" \
    -max_len=4096 \
    -timeout=10 \
    -rss_limit_mb=4096 \
    -print_final_stats=1

echo "std.sync collection fuzz: OK ($runs runs, seed $seed, bounded model oracle)"
