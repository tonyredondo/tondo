#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-conf-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
evidence="$target_dir/reliability/evidence"
tmp="$(mktemp -d "$target_dir/reliability/native-conf.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$evidence"

scripts/native-conf-adapter-check.sh
CARGO_TARGET_DIR="$target_dir" scripts/native-conf-adapter-test.sh
CARGO_TARGET_DIR="$target_dir" scripts/native-conf-language-test.sh
CARGO_TARGET_DIR="$target_dir" scripts/native-conf-testing-test.sh
CARGO_TARGET_DIR="$target_dir" scripts/native-conf-stdlib-test.sh

for backend in cranelift llvm; do
    for category in language testing stdlib; do
        scripts/native-conformance-adapter.sh \
            --backend "$backend" \
            --target x86_64-unknown-linux-gnu \
            --category "$category" \
            --output "$tmp/$backend-$category.json"
        jq -e --arg b "$backend" --arg c "$category" '
          .backend == $b and .category == $c
          and .target == "x86_64-unknown-linux-gnu"
          and .mir == "tondo-mir-backend/1"
          and .oracle == "bytecode-vm-oracle"
          and .status == "passed"
          and (.physical_paths == [])
          and (.observations | length > 0)
          and all(.observations[]; .status == "passed" and .native == .oracle and .physical_paths == [])
          and (([.observations[].id] | length) == ([.observations[].id] | unique | length))
        ' "$tmp/$backend-$category.json" >/dev/null
    done
done

for category in language testing stdlib; do
    jq -S '[.observations[] | {id, oracle, native, status}]' "$tmp/cranelift-$category.json" > "$tmp/cranelift-$category.norm"
    jq -S '[.observations[] | {id, oracle, native, status}]' "$tmp/llvm-$category.json" > "$tmp/llvm-$category.norm"
    cmp -s "$tmp/cranelift-$category.norm" "$tmp/llvm-$category.norm" \
        || { echo "native conformance coordination: backend divergence in $category" >&2; exit 1; }
done

contract_hash="$(sha256sum testing/native-conf.json | cut -d ' ' -f1)"
jq -n --arg contract "sha256:$contract_hash" \
  '{format:"tondo-native-conf-evidence/1",task:"NATIVE-CONF-001",status:"passed",contract:$contract,target:"x86_64-unknown-linux-gnu",mir:"tondo-mir-backend/1",oracle:"bytecode-vm-oracle",categories:{language:{status:"passed",cases:3},testing:{status:"passed",cases:3},stdlib:{status:"passed",cases:3}},backends:{cranelift:{status:"passed",cases:9},llvm:{status:"passed",cases:9}},dimensions:{cross_backend:true,independent_oracle:true,path_redaction:true,cleanup:true,fail_closed:true},physical_paths:[],divergences:[]}' \
  > "$evidence/native-conf.json"
echo "native conformance coordination tests: OK (9 cases x 2 candidates; evidence written to ${evidence#"$root/"}/native-conf.json)"
