#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_HOST_CONTRACT:-testing/stdlib-net-host.json}"
bash scripts/stdlib-net-check.sh >/dev/null
bash scripts/async-select-transactions-check.sh >/dev/null
jq -e --slurpfile parent testing/stdlib-net.json '
  .format == "tondo-stdlib-host-boundary/1" and .owner == "std.net"
  and .task == "STD-NET-HOST-001" and .prerequisite == "ASYNC-SELECT-ATOMIC-001"
  and .target == "tondo-vm-hosted" and .selected_route == "hosted-scalar"
  and .public_api_promoted == false and .native_abi_or_aot == false
  and (.status == "implementation-in-progress" or .status == "verified-production-hosted")
  and (if .status == "implementation-in-progress" then .quality_gate == "pending-current-source-proof"
       else .quality_gate == "verified-80-percent-per-scope" end)
  and .status == $parent[0].host.status
  and .quality_gate == $parent[0].host.quality_gate
  and .surface == {operations:40, value_types:15, host_carriers:12, address_records:3, suspendible:15, selectable:3}
  and .providers.resolver == "explicit-ordered-target-endpoints-no-ambient-config"
  and .providers.tls_versions == ["1.2", "1.3"]
  and .providers.roots == "versioned-webpki-roots-bundle"
  and .providers.import_effects == false and .providers.blocking_fallback == false
  and .providers.hosted_spawn_thread == "cooperative-reference"
  and .admission.tls_consumes_tcp_on_error
  and .admission.known_error == "fixed-envelope-before-success-workspace"
  and .admission.memory_metric == "logical-not-rss-or-os-allocation-count"
  and (.fixtures | length) == 4 and (.tests | length) == 15
  and (.tests | unique | length) == 15
' "$contract" >/dev/null
while IFS= read -r path; do test -f "$path"; done < <(jq -r '.fixtures[], .contract' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    rg -Fq "fn $name(" "$path"
done < <(jq -r '.tests[]' "$contract")
echo 'std.net public hosted contract: OK'
