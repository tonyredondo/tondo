#!/usr/bin/env bash
set -euo pipefail

root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
nightly="${TONDO_CBOR_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_CBOR_FUZZ_RUNS:-128}"
seed="${TONDO_CBOR_FUZZ_SEED:-4113}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "std.cbor fuzz: invalid run count" >&2; exit 1; }
[[ "$seed" =~ ^[0-9]+$ ]] || { echo "std.cbor fuzz: invalid seed" >&2; exit 1; }
export CARGO_TARGET_DIR="$target_dir"

# Each smoke starts from exactly the retained wire vectors and seed; generated
# corpus entries from an earlier campaign cannot change this replay's input.
parent="$target_dir/reliability/fuzz-corpus"
mkdir -p "$parent"
campaign="$(mktemp -d "$parent/stdlib-cbor.XXXXXX")"
mkdir -p "$campaign/input" "$campaign/output"
python3 - "$root/crates/tondo-reliability/tests/fixtures/cbor-wire.json" "$campaign/input" <<'PY'
import json
import pathlib
import sys
corpus = json.loads(pathlib.Path(sys.argv[1]).read_text())
output = pathlib.Path(sys.argv[2])
for fixture in corpus["valid"] + corpus["invalid"]:
    (output / fixture["id"]).write_bytes(bytes.fromhex(fixture["wire"]))
PY
cd "$root/fuzz"
cargo "+$nightly" fuzz run --no-default-features stdlib_cbor \
    "$campaign/output" "$campaign/input" corpus/stdlib_cbor -- \
    "-runs=$runs" "-seed=$seed" -max_len=4096 -timeout=10 \
    -rss_limit_mb=4096 -print_final_stats=1
echo "std.cbor fuzz: OK ($runs runs, seed $seed, independent wire/encoding oracle)"
