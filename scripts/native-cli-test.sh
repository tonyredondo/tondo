#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-cli-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
bin="${TONDO_BIN:-$target_dir/debug/tondo}"
if [[ -x "$bin" ]]; then
    tondo() { "$bin" "$@"; }
else
    # Cargo's progress/status lines use stderr.  Keep them out of the CLI's
    # stderr contract so the fallback behaves exactly like an existing binary.
    tondo() { CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-cli --locked --quiet -- "$@"; }
fi

tmp_root="$target_dir/reliability/native-cli"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/project.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$tmp/src" "$tmp/tests"
cp tests/native/native-cli-001.to "$tmp/src/main.to"
cp tests/native/native-cli-001.to "$tmp/tests/smoke.to"

cp acceptance/projects/testing-acceptance/tondo.toml "$tmp/tondo.toml"

tondo build --project "$tmp"
[[ -s "$tmp/build/tondo.artifact.json" ]]
[[ -s "$tmp/build/tondo.artifact.native.json" ]]
jq -e '.format == "tondo-native-build/1" and .status == "selection-pending" and .candidates == ["cranelift","llvm"] and .ambient_lookup == false' \
    "$tmp/build/tondo.artifact.native.json" >/dev/null
cp "$tmp/build/tondo.artifact.json" "$tmp/artifact-one"
cp "$tmp/build/tondo.artifact.native.json" "$tmp/native-one"

tondo build --project "$tmp"
cmp -s "$tmp/artifact-one" "$tmp/build/tondo.artifact.json"
cmp -s "$tmp/native-one" "$tmp/build/tondo.artifact.native.json"

output="$(tondo run --project "$tmp" -- --cli-argument 2>"$tmp/stderr")"
[[ "$output" = "tondo-cli" ]]
[[ ! -s "$tmp/stderr" ]]

if tondo build --project "$tmp" --native >/dev/null 2>&1; then
    echo "native CLI tests: forbidden --native option unexpectedly accepted" >&2
    exit 1
fi

echo "native CLI tests: OK (build/run project lifecycle and failure boundaries)"
