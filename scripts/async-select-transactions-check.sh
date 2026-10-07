#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_SELECT_TRANSACTIONS_CONTRACT:-testing/async-select-transactions.json}"
jq -e '
  .format == "tondo-selection-transactions/1"
  and .task == "ASYNC-SELECT-ATOMIC-001"
  and .target == "tondo-vm-hosted" and .native_abi_or_aot == false
  and (.status == "implementation-in-progress" or .status == "verified-production-hosted")
  and (if .status == "implementation-in-progress" then .quality_gate == "pending-current-source-proof"
       else .quality_gate == "verified-80-percent-per-scope" end)
  and .protocol == {
    registration_identity:true, seal_before_foreign_rendezvous:true,
    self_rendezvous:false, joint_result_admission:true, joint_winner_claim:true,
    loser_consumes_payload:false, affine_transfer:"winner-edge-only",
    source_checkpoint:"before-continuation", source_prefix_diagnostic:"E1614",
    artifact_prefix_verification:true,
    forced_winner_cancel:"terminal-cleanup-of-transferred-owners",
    memory_metric:"logical-not-rss-or-os-allocation-count"
  }
  and .model.seeds == 4096 and (.tests | length) == 18
  and (.tests | unique | length) == 18
' "$contract" >/dev/null
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path"
done < <(jq -r '.tests[], (.model.source + "::" + .model.test)' "$contract")
test -f "$(jq -r '.contract' "$contract")"
echo 'hosted selection transaction contract: OK'
