#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
nightly="${TONDO_UUID_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_UUID_FUZZ_RUNS:-128}"
seed="${TONDO_UUID_FUZZ_SEED:-4113}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "std.uuid fuzz: invalid run count" >&2; exit 1; }
[[ "$seed" =~ ^[0-9]+$ ]] || { echo "std.uuid fuzz: invalid seed" >&2; exit 1; }
export CARGO_TARGET_DIR="$target_dir"
parent="$target_dir/reliability/fuzz-corpus"
mkdir -p "$parent"
campaign="$(mktemp -d "$parent/stdlib-uuid.XXXXXX")"
mkdir -p "$campaign/input" "$campaign/output"
python3 - "$root/crates/tondo-reliability/tests/fixtures/uuid-cases.json" "$campaign/input" <<'PY'
import json
import pathlib
import sys
corpus = json.loads(pathlib.Path(sys.argv[1]).read_text())
output = pathlib.Path(sys.argv[2])
for fixture in corpus["valid"] + corpus["invalid"]:
    operation = fixture["operation"]
    if operation == "parse":
        data = b"\0" + fixture["text"].encode("utf-8")
    elif operation == "bytes":
        data = b"\1" + bytes.fromhex(fixture["bytes_hex"])
    elif operation == "v4":
        data = b"\2" + bytes.fromhex(fixture["entropy_hex"])
    elif operation == "v5":
        namespace = bytes.fromhex(fixture["namespace"].replace("-", ""))
        data = b"\3" + fixture["name_limit"].to_bytes(2, "big") + namespace + bytes.fromhex(fixture["name_hex"])
    elif operation == "v7":
        data = b"\4" + fixture["milliseconds"].to_bytes(16, "big", signed=True) + bytes.fromhex(fixture["entropy_hex"])
    else:
        raise ValueError(operation)
    (output / fixture["id"]).write_bytes(data)
PY
cd "$root/fuzz"
cargo "+$nightly" fuzz run --no-default-features stdlib_uuid \
    "$campaign/output" "$campaign/input" corpus/stdlib_uuid -- \
    "-runs=$runs" "-seed=$seed" -max_len=4096 -timeout=10 \
    -rss_limit_mb=4096 -print_final_stats=1
echo "std.uuid fuzz: OK ($runs runs, seed $seed, independent bounded UUID oracle)"
