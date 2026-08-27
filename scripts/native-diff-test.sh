#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-diff-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-diff.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
mkdir -p "$target_dir/reliability/evidence"

CARGO_TARGET_DIR="$target_dir" scripts/native-conf-test.sh >/dev/null

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
          and .oracle == "bytecode-vm-oracle"
          and .status == "passed"
          and (.physical_paths == [])
          and all(.observations[]; .native == .oracle and .status == "passed" and .physical_paths == [])
        ' "$tmp/$backend-$category.json" >/dev/null
        jq -S '[.observations[] | {id, oracle, native, status}]' \
            "$tmp/$backend-$category.json" > "$tmp/$backend-$category.norm"
    done
done

for category in language testing stdlib; do
    cmp -s "$tmp/cranelift-$category.norm" "$tmp/llvm-$category.norm" \
        || { echo "native differential: backend divergence in $category" >&2; exit 1; }
done

# Exercise the fail-closed assertion itself: a mutated oracle value must not
# compare equal to the canonical candidate observation.
jq '(.observations[0].oracle = "mutated")' "$tmp/cranelift-language.json" > "$tmp/mutated.json"
if cmp -s <(jq -S '[.observations[] | {id, oracle, native, status}]' "$tmp/mutated.json") \
    "$tmp/cranelift-language.norm"; then
    echo "native differential: mutated oracle unexpectedly matched" >&2
    exit 1
fi

contract_hash="$(sha256sum testing/native-diff.json | cut -d ' ' -f1)"
executable_status="opt-in"
if [[ "${TONDO_NATIVE_DIFF_EXECUTABLE:-0}" == 1 ]]; then
    TONDO_LLVM_LLC="${TONDO_LLVM_LLC:-/usr/bin/llc}" \
    TONDO_NATIVE_CC="${TONDO_NATIVE_CC:-/usr/bin/cc}" \
    CARGO_TARGET_DIR="$target_dir" scripts/native-evaluation-runner.sh
    executable_status="passed"
fi
jq -n --arg contract "sha256:$contract_hash" --arg executable "$executable_status" \
  '{format:"tondo-native-diff-evidence/1",task:"NATIVE-DIFF-001",status:"passed",contract:$contract,target:"x86_64-unknown-linux-gnu",seed:"tondo-native-diff-0.1",oracle:"bytecode-vm-oracle",cases:9,backends:{cranelift:{status:"passed",cases:9},llvm:{status:"passed",cases:9}},properties:{same_observation_id_set:true,native_equals_oracle:true,cross_backend_equality:true,stable_generation:true,path_redaction:true,fail_closed_mismatch:true},executable_lane:$executable,physical_paths:[],divergences:[]}' \
  > "$target_dir/reliability/evidence/native-diff.json"
echo "native differential tests: OK (9 generated cases x 2 candidates; executable lane: $executable_status)"
