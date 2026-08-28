#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-conf-language-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-conf-language.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
for backend in cranelift llvm; do
    scripts/native-conformance-adapter.sh \
        --backend "$backend" \
        --target x86_64-unknown-linux-gnu \
        --category language \
        --output "$tmp/$backend.json"
    jq -e --arg b "$backend" '
      .backend == $b and .category == "language" and .status == "passed"
      and ([.observations[].id] | sort) == ["language-panic", "language-result-error", "language-scalar"]
      and ([.observations[] | select(.native == .oracle and .status == "passed")] | length == 3)
    ' "$tmp/$backend.json" >/dev/null
done
contract_hash="$(sha256sum testing/native-conf-language.json | cut -d ' ' -f1)"
mkdir -p "$target_dir/reliability/evidence"
jq -n --arg contract "sha256:$contract_hash" \
  '{format:"tondo-native-conf-language-evidence/1",task:"NATIVE-CONF-LANGUAGE-001",status:"passed",contract:$contract,target:"x86_64-unknown-linux-gnu",backends:{cranelift:{status:"passed",cases:3},llvm:{status:"passed",cases:3}},oracle:"bytecode-vm-oracle",physical_paths:[],divergences:[]}' \
  > "$target_dir/reliability/evidence/native-conf-language.json"
echo "native language conformance tests: OK (3 cases x 2 candidates)"
