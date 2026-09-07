#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-owner-evidence.json}"
source="${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}"
contract="${TONDO_STDLIB_FUZZ_CONTRACT:-testing/stdlib-fuzz.json}"
conformance="${TONDO_STDLIB_CONFORMANCE_CONTRACT:-testing/stdlib-conformance.json}"

scripts/stdlib-fuzz-check.sh --contract-only >/dev/null
generated="$(mktemp "${TMPDIR:-/tmp}/tondo-owner-evidence.XXXXXX")"
trap 'rm -f "$generated"' EXIT

# Other cells remain authored component evidence. FUZZ and CONF are projections
# of their current contracts; a declared case is not an observed execution.
jq --slurpfile fuzz "$contract" --slurpfile conformance "$conformance" '
  . as $registry
  | if ([$registry.owners[].id] | sort) != ([$fuzz[0].owners[].id] | sort)
       or ([$registry.owners[].id] | sort) != ([$conformance[0].owners[].id] | sort)
       or $conformance[0].status != "planned"
       or any($conformance[0].owners[]; .status != "pending")
    then error("owner contracts do not match the current component boundary") else . end
  | .status = "component-evidence"
  | .promotion = "pending"
  | .owners |= map(. as $owner
      | first($fuzz[0].owners[] | select(.id == $owner.id)) as $route
      | first($conformance[0].owners[] | select(.id == $owner.id)) as $conf
      | .cells.FUZZ = {
          status: $route.status, reason: $route.reason,
          evidence_kind: $route.evidence_kind, component_status: $route.component_status,
          refs: ["fuzz/fuzz_targets/stdlib_owners.rs#route=" + $owner.id,
                 $route.corpus + "#seed=" + ($route.seed | tostring),
                 "testing/stdlib-fuzz.json#owner=" + $owner.id,
                 "scripts/stdlib-fuzz-check.sh", "scripts/fuzz-smoke.sh", "scripts/fuzz-campaign.sh"]
        }
      | .cells.CONF.status = $conf.status
      | .cells.CONF.reason = $conf.reason
      | .cells.CONF.scope = $conf.scope
      | .cells.CONF.refs |= ((. + ["testing/stdlib-conformance.json#owners/" + $owner.id]) | unique))
' "$source" > "$generated"
cat "$generated" > "$output"
echo "stdlib owner evidence synchronized: $output (promotion pending)"
