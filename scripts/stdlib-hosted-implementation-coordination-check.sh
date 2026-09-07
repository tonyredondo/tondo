#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

coordination="${TONDO_STDLIB_HOSTED_IMPLEMENTATION_COORDINATION:-testing/stdlib-hosted-implementation-coordination.json}"
[[ -f "$coordination" ]] || {
    echo "stdlib hosted implementation coordination: missing registry: ${coordination#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-hosted-implementation-coordination.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-hosted-implementation-coordination-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$coordination" || {
    echo "stdlib hosted implementation coordination: registry is stale; run scripts/stdlib-hosted-implementation-coordination-generate.sh" >&2
    exit 1
}

jq -e --slurpfile api "${TONDO_STDLIB_PUBLIC_API:-testing/stdlib-public-api.json}" '
  def public_valid:
    .public_api as $api
    | ($api.signature_count == ($api.signatures | length))
      and ($api.verified_count == ([$api.signatures[] | select(.status == "verified")] | length))
      and ($api.gap_count == $api.signature_count - $api.verified_count)
      and ($api.owner_missing | type == "array")
      and ($api.status == (if $api.owner_status == "verified" and ($api.owner_missing | length) == 0 and $api.signature_count > 0 and $api.gap_count == 0 then "verified" else "open-gaps" end));
  def owner_closed:
    .implementation_status == "verified"
    and .matrix_impl_host == "verified"
    and .public_api.status == "verified"
    and (.host.status == "verified" or (.id == "std.path" and .host.status == "not-applicable"));
  .format == "tondo-stdlib-hosted-implementation-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == (if all(.owners[]; owner_closed) then "closed-coordination" else "open-gaps" end)
  and .sources.owner_contract == "testing/stdlib-hosted.json"
  and .sources.implementation == "testing/stdlib-implementation.json"
  and .sources.public_api == "testing/stdlib-public-api.json"
  and .sources.normative_matrix == "testing/stdlib-matrix.json"
  and .sources.owner_evidence == "testing/stdlib-owner-evidence.json"
  and .rules.required_owner_status == "implemented-draft"
  and .rules.implementation_stage_must_be_verified
  and .rules.hosted_stage_must_be_verified_or_not_applicable
  and .rules.capability_boundary_must_match_contract
  and .rules.callable_public_signatures_must_be_verified
  and .rules.empty_public_surface_is_unverified
  and .rules.global_public_audit_is_not_promoted
  and .rules.no_waivers
  and .next_coordination == "STD-B-INTEGRATION-PLAN-001"
  and .global_public_api.status == $api[0].status
  and .global_public_api.gaps == $api[0].summary.gaps
  and .summary == {
    owners: (.owners | length),
    signatures: ([.owners[].public_api.signatures[]] | length),
    verified_signatures: ([.owners[].public_api.signatures[] | select(.status == "verified")] | length),
    owners_with_public_gaps: ([.owners[] | select(.public_api.status != "verified")] | length),
    capability_gated_owners: ([.owners[] | select((.capability | length) > 0)] | length),
    pure_owners: ([.owners[] | select((.capability | length) == 0)] | length),
    host_verified_owners: ([.owners[] | select(.host.status == "verified")] | length),
    host_not_applicable_owners: ([.owners[] | select(.host.status == "not-applicable")] | length)
  }
  and (.owners | map(.id)) == ["std.console", "std.path", "std.fs", "std.process"]
  and all(.owners[];
    (.layer == "A2")
    and (.implementation_status == "verified" or .implementation_status == "open-gaps")
    and (.implementation | type == "array" and length > 0)
    and (.tests | type == "array" and length > 0)
    and (.proof | type == "string" and length > 0)
    and (.capability | type == "array")
    and public_valid
    and (if .id == "std.path" then
          (.capability == [] and (.host.reason | type == "string" and length > 0))
        elif .id == "std.console" then .capability == ["console"]
        elif .id == "std.fs" then .capability == ["filesystem"]
        elif .id == "std.process" then .capability == ["process"]
        else false end)
  )
' "$coordination" >/dev/null || {
    echo "stdlib hosted implementation coordination: invalid registry" >&2
    exit 1
}

while IFS= read -r path; do
    [[ -e "$root/$path" ]] || {
        echo "stdlib hosted implementation coordination: missing evidence path: $path" >&2
        exit 1
    }
done < <(jq -r '.owners[] | .implementation[], .tests[], .host.refs[]' "$coordination")

for ref in \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    docs/contracts/stdlib-hosted.md \
    docs/contracts/stdlib-s1a.md \
    testing/stdlib-hosted.json \
    testing/stdlib-implementation.json \
    testing/stdlib-public-api.json \
    testing/stdlib-matrix.json \
    testing/stdlib-owner-evidence.json; do
    [[ -e "$root/$ref" ]] || {
        echo "stdlib hosted implementation coordination: missing source reference: $ref" >&2
        exit 1
    }
done

echo "stdlib hosted implementation coordination: consistent ($(jq -r '.status' "$coordination")); global API gaps: $(jq -r '.global_public_api.gaps' "$coordination")"
