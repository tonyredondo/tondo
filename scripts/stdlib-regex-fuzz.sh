#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
nightly="${TONDO_REGEX_FUZZ_NIGHTLY:-nightly-2026-07-28}"
runs="${TONDO_REGEX_FUZZ_RUNS:-128}"
seed="${TONDO_REGEX_FUZZ_SEED:-4113}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "std.regex fuzz: invalid run count" >&2; exit 1; }
[[ "$seed" =~ ^[0-9]+$ ]] || { echo "std.regex fuzz: invalid seed" >&2; exit 1; }
export CARGO_TARGET_DIR="$target_dir"
parent="$target_dir/reliability/fuzz-corpus"
mkdir -p "$parent"
campaign="$(mktemp -d "$parent/stdlib-regex.XXXXXX")"
mkdir -p "$campaign/input" "$campaign/output"
python3 - "$root/crates/tondo-reliability/tests/fixtures/regex-cases.json" "$campaign/input" <<'PY'
import json
import pathlib
import sys
corpus = json.loads(pathlib.Path(sys.argv[1]).read_text())
output = pathlib.Path(sys.argv[2])
for fixture in corpus["valid"] + corpus["invalid"]:
    replacement = fixture.get("replacement", {}).get("template", "[$0]$$")
    data = bytes([fixture.get("options", 0)]) + b"\0".join(
        value.encode("utf-8") for value in (fixture["pattern"], fixture.get("input", ""), replacement)
    )
    (output / fixture["id"]).write_bytes(data)
PY
cd "$root/fuzz"
cargo "+$nightly" fuzz run --no-default-features stdlib_regex \
    "$campaign/output" "$campaign/input" corpus/stdlib_regex -- \
    "-runs=$runs" "-seed=$seed" -max_len=4096 -timeout=10 \
    -rss_limit_mb=4096 -print_final_stats=1
echo "std.regex fuzz: OK ($runs runs, seed $seed, independent ordered-path oracle)"
