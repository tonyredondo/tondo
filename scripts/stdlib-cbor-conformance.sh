#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/stdlib-cbor-conformance-check.sh
contract="${TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT:-testing/stdlib-cbor-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
export CARGO_TARGET_DIR="$target_dir"
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
mkdir -p "$evidence_dir"
python3 -B scripts/stdlib_cbor_conformance.py capture --contract "$contract" --report "$evidence_dir/stdlib-cbor-conformance.json"
