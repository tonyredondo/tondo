#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

coordination="${TONDO_STDLIB_IMPLEMENTATION_COORDINATION:-testing/stdlib-implementation-coordination.json}"
[[ -f "$coordination" ]] || {
    echo "stdlib implementation coordination: missing registry: ${coordination#"$root"/}" >&2
    exit 1
}

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-implementation-coordination.XXXXXX.json")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-implementation-coordination-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$coordination" || {
    echo "stdlib implementation coordination: registry is stale; run scripts/stdlib-implementation-coordination-generate.sh" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-implementation-coordination/1"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "closed-coordination"
  and .sources.implementation == "testing/stdlib-implementation.json"
  and .sources.public_api == "testing/stdlib-public-api.json"
  and .sources.normative_matrix == "testing/stdlib-matrix.json"
  and .rules.required_owner_status == "implemented-draft"
  and .rules.implementation_stage_must_be_verified
  and .rules.callable_public_signatures_must_be_verified
  and .rules.build_only_no_callable_surface_requires_reason
  and .rules.global_public_audit_is_not_promoted
  and .rules.no_waivers
  and .next_coordination == "STD-IMPL-002"
  and .global_public_api.status == "open-gaps"
  and (.global_public_api.gaps | type == "number" and . > 0)
  and .summary == {
    owners: 8,
    signatures: 64,
    verified_signatures: 64,
    owners_with_public_surface: 7,
    owners_without_callable_surface: 1
  }
  and (.owners | map(.id)) == ["std.core", "std.text", "std.collections", "std.iter", "std.math", "std.format", "std.io", "std.serialization"]
  and all(.owners[];
    (.layer | test("^A[13]$"))
    and (.implementation_status == "verified")
    and (.matrix_impl_host == "verified")
    and (.implementation | type == "array" and length > 0)
    and (.tests | type == "array" and length > 0)
    and (.proof | type == "string" and length > 0)
    and (if .public_api.status == "verified" then
          (.public_api.signature_count > 0
           and .public_api.signature_count == .public_api.verified_count
           and .public_api.gap_count == 0
           and all(.public_api.signatures[]; .status == "verified"))
        elif .public_api.status == "not-applicable" then
          (.id == "std.serialization"
           and (.public_surface_reason | type == "string" and length > 0)
           and .public_api.signature_count == 0)
        else false end)
  )
' "$coordination" >/dev/null || {
    echo "stdlib implementation coordination: invalid registry" >&2
    exit 1
}

while IFS= read -r path; do
    [[ -e "$root/$path" ]] || {
        echo "stdlib implementation coordination: missing evidence path: $path" >&2
        exit 1
    }
done < <(jq -r '.owners[] | .implementation[], .tests[]' "$coordination")

while IFS= read -r ref; do
    [[ -e "$root/$ref" ]] || {
        echo "stdlib implementation coordination: missing source reference: $ref" >&2
        exit 1
    }
done < <(jq -r '.sources[]' "$coordination")

echo "stdlib implementation coordination: OK (8 owners; 64 public signatures verified; codec/build-only gaps explicit; next STD-IMPL-002)"
