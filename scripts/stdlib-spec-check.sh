#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="testing/stdlib-spec.json"
[[ -f "$contract" ]] || { echo "missing stdlib integration contract" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || { echo "stdlib integration contract must end with LF" >&2; exit 1; }
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || { echo "stdlib integration contract has whitespace" >&2; exit 1; }

jq -e '
  .format == "tondo-stdlib-integration-contract/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-draft"
  and .public_release == false
  and .canonical_spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .owner_manifest == "testing/stdlib-implementation.json"
  and .intrinsics == ["Bytes"]
  and (.owner_contracts | length) == 21
  and ([.owner_contracts[].id] | unique | length) == 21
  and (.owner_contracts | map(.contract) | length) == 21
  and ((.owner_contracts | map(.contract) | unique | length) < (.owner_contracts | length))
  and .capability_rules.import_is_not_capability == true
  and .capability_rules.ambient_lookup == false
  and .api_rules.canonical_owner_per_symbol == true
  and .api_rules.duplicate_signatures == false
  and .api_rules.aliases == false
  and .api_rules.implicit_defaults == false
  and .api_rules.error_model == "nominal-result-per-owner"
  and .api_rules.async_io_model == "std.io-reader-writer"
  and .promotion.required_owner_status == "closed-contract"
  and .promotion.implementation_remains_pending == ["std.async"]
  and .promotion.next == "NATIVE-LINK-PLAN-001"
' "$contract" >/dev/null

expected='std.meta std.reflect std.core std.time std.env std.text std.collections std.iter std.math std.format std.io std.async std.serialization std.path std.console std.fs std.process std.json std.messagepack std.protobuf std.testing'
actual="$(jq -r '.owner_contracts[].id' "$contract" | paste -sd ' ' -)"
[[ "$actual" == "$expected" ]] || { echo "stdlib owner order is not canonical" >&2; exit 1; }

manifest_ids="$(jq -r '.owners[].id' testing/stdlib-implementation.json | sort | paste -sd ' ' -)"
contract_ids="$(jq -r '.owner_contracts[].id' "$contract" | sort | paste -sd ' ' -)"
[[ "$manifest_ids" == "$contract_ids" ]] || { echo "integration owners do not match implementation manifest" >&2; exit 1; }

while IFS= read -r path; do
    [[ -e "$path" ]] || { echo "missing stdlib contract path: $path" >&2; exit 1; }
done < <(jq -r '.owner_contracts[].contract' "$contract" | sort -u)

# The declared order is a topological order: every dependency must precede its
# consumer. This proves the closed graph has no cycle without a second graph
# implementation in the gate.
while IFS=$'\t' read -r owner dependencies; do
    owner_index=$(jq -r --arg owner "$owner" '.owner_contracts | map(.id) | index($owner)' "$contract")
    for dependency in $dependencies; do
        dependency_index=$(jq -r --arg dependency "$dependency" '.owner_contracts | map(.id) | index($dependency)' "$contract")
        [[ "$dependency_index" != "null" && "$dependency_index" -lt "$owner_index" ]] || {
            echo "dependency order/cycle violation: $owner -> $dependency" >&2
            exit 1
        }
    done
done < <(jq -r '.owner_contracts[] | [.id, (.dependencies | join(" "))] | @tsv' "$contract")

for path in TONDO_STANDARD_LIBRARY_SPEC.md docs/contracts/stdlib-s1a.md; do
    grep -Fq 'testing/stdlib-spec.json' "$path" || {
        echo "integration contract is not linked from $path" >&2
        exit 1
    }
    grep -Fq 'stdlib-public-api-audit.md' "$path" || {
        echo "public API audit is not linked from $path" >&2
        exit 1
    }
done

echo "stdlib integration contract: OK"
