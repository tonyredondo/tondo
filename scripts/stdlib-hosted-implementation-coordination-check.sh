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

jq -e '
  .format == "tondo-stdlib-hosted-implementation-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-coordination"
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
  and .rules.global_public_audit_is_not_promoted
  and .rules.no_waivers
  and .next_coordination == "NATIVE-LINK-PLAN-001"
  and .global_public_api.status == "verified"
  and .global_public_api.gaps == 0
  and .summary == {
    owners: 4,
    signatures: 48,
    verified_signatures: 48,
    capability_gated_owners: 3,
    pure_owners: 1,
    host_verified_owners: 3,
    host_not_applicable_owners: 1
  }
  and (.owners | map(.id)) == ["std.console", "std.path", "std.fs", "std.process"]
  and all(.owners[];
    (.layer == "A2")
    and (.implementation_status == "verified")
    and (.matrix_impl_host == "verified")
    and (.implementation | type == "array" and length > 0)
    and (.tests | type == "array" and length > 0)
    and (.proof | type == "string" and length > 0)
    and (.capability | type == "array")
    and (.public_api.status == "verified")
    and (.public_api.signature_count > 0)
    and (.public_api.signature_count == .public_api.verified_count)
    and (.public_api.gap_count == 0)
    and all(.public_api.signatures[]; .status == "verified")
    and (if .id == "std.path" then
          (.capability == []
           and .host.status == "not-applicable"
           and (.host.reason | type == "string" and length > 0))
        elif .id == "std.console" then
          (.capability == ["console"] and .host.status == "verified")
        elif .id == "std.fs" then
          (.capability == ["filesystem"] and .host.status == "verified")
        elif .id == "std.process" then
          (.capability == ["process"] and .host.status == "verified")
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

echo "stdlib hosted implementation coordination: OK (4 owners; 48 public signatures verified; capabilities and hosted bridges closed; next NATIVE-LINK-PLAN-001)"
