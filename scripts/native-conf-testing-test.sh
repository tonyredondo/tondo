#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/native-conf-testing-check.sh
target_dir="${CARGO_TARGET_DIR:-$root/target-fast}"
tmp="$(mktemp -d "$target_dir/reliability/native-conf-testing.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
for backend in cranelift llvm; do
    scripts/native-conformance-adapter.sh --backend "$backend" --target x86_64-unknown-linux-gnu --category testing --output "$tmp/$backend.json"
    jq -e --arg b "$backend" '
      .backend == $b and .category == "testing" and .status == "passed"
      and ([.observations[].id] | sort) == ["testing-fail", "testing-isolation", "testing-pass"]
      and ([.observations[] | select(.native == .oracle and .status == "passed")] | length == 3)
      and ([.observations[].oracle | select(.diagnostic == "P0007")] | length == 1)
    ' "$tmp/$backend.json" >/dev/null
done
contract_hash="$(sha256sum testing/native-conf-testing.json | cut -d ' ' -f1)"
mkdir -p "$target_dir/reliability/evidence"
jq -n --arg contract "sha256:$contract_hash" \
  '{format:"tondo-native-conf-testing-evidence/1",task:"NATIVE-CONF-TESTING-001",status:"passed",contract:$contract,target:"x86_64-unknown-linux-gnu",backends:{cranelift:{status:"passed",cases:3},llvm:{status:"passed",cases:3}},oracle:"bytecode-vm-oracle",lifecycle:{logs:true,failure_code:"P0007",cleanup:1,fresh_process:true},physical_paths:[],divergences:[]}' \
  > "$target_dir/reliability/evidence/native-conf-testing.json"
echo "native testing conformance tests: OK (3 lifecycle cases x 2 candidates)"
