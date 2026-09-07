#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_TOML_CONTRACT:-$root/testing/stdlib-toml.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-toml-implementation-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.toml implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_TOML_CONTRACT="$contract" scripts/stdlib-toml-implementation-check.sh

stdlib_test_log="$logs_dir/stdlib-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib --locked toml::tests \
    >"$stdlib_test_log" 2>&1 \
    || { cat "$stdlib_test_log" >&2; die "TOML stdlib tests failed"; }

clippy_log="$logs_dir/clippy.log"
CARGO_TARGET_DIR="$target_dir" cargo clippy -q -p tondo-stdlib --locked -- -D warnings \
    >"$clippy_log" 2>&1 \
    || { cat "$clippy_log" >&2; die "TOML stdlib clippy failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d" " -f1)"
stdlib_test_sha256="$(sha256sum "$stdlib_test_log" | cut -d" " -f1)"
clippy_sha256="$(sha256sum "$clippy_log" | cut -d" " -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg stdlib_test_sha256 "$stdlib_test_sha256" \
    --arg clippy_sha256 "$clippy_sha256" \
    '
      {
        format:"tondo-stdlib-toml-implementation-evidence/1",
        task:"STD-TOML-IMPL-001",
        status:"passed",
        source_revision:$revision,
        contract_sha256:("sha256:" + $contract_sha256),
        scalar_tests:{package:"tondo-stdlib",filter:"toml::tests",status:"passed",log_sha256:("sha256:" + $stdlib_test_sha256)},
        clippy:{package:"tondo-stdlib",status:"passed",log_sha256:("sha256:" + $clippy_sha256)},
        public_boundary:{api_promoted:false,host:"not-claimed-until-compiler-toml-abi",native_runtime:"not-claimed",native_aot_lowering:"not-claimed"},
        fixture:null,
        physical_paths:[],
        timestamps:false,
        addresses:[],
        divergences:[]
      }
    ' >"$evidence_dir/stdlib-toml-implementation.json"

echo "std.toml implementation: OK (stdlib kernel; report: $evidence_dir/stdlib-toml-implementation.json)"
