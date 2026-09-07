#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-implementation-coordination.json}"
mkdir -p "$(dirname "$output")"

# STD-IMPL-001 is deliberately a coordinator for the already specified Core
# group plus the shared serialization kernel. A component record cannot
# waive an unindexed or incomplete public surface.
jq -n \
    --slurpfile implementation testing/stdlib-implementation.json \
    --slurpfile api "${TONDO_STDLIB_PUBLIC_API:-testing/stdlib-public-api.json}" \
    --slurpfile matrix "${TONDO_STDLIB_MATRIX:-testing/stdlib-matrix.json}" \
    --slurpfile evidence "${TONDO_STDLIB_OWNER_EVIDENCE:-testing/stdlib-owner-evidence.json}" \
    '
    ($implementation[0]) as $implementation
    | ($api[0]) as $api
    | ($matrix[0]) as $matrix
    | ($evidence[0]) as $evidence
    | ["std.core", "std.text", "std.collections", "std.iter", "std.math", "std.format", "std.io", "std.serialization"] as $required
    | ($required | map(. as $id
        | (first($implementation.owners[] | select(.id == $id)) // null) as $owner
        | (first($matrix.owners[] | select(.id == $id)) // null) as $matrix_owner
        | (first($evidence.owners[] | select(.id == $id)) // null) as $evidence_owner
        | (first($api.owners[] | select(.id == $id)) // null) as $api_owner
        | ([ $api.rows[] | select(.owner == $id) ] | sort_by(.line)) as $signatures
        | {
            id: $id,
            layer: $owner.layer,
            implementation: $owner.implementation,
            tests: $owner.tests,
            proof: $owner.proof,
            implementation_status: (if ($owner.implementation | length) > 0 and ($owner.tests | length) > 0 and $evidence_owner.cells.IMPL.status == "verified" then "verified" else "open-gaps" end),
            matrix_impl_host: $matrix_owner.stages["IMPL/HOST"].status,
            public_api: {
                status: (if $api_owner.status == "verified" and ($api_owner.owner_missing | length) == 0 and ($signatures | length) > 0 and all($signatures[]; .status == "verified") then "verified" else "open-gaps" end),
                owner_status: ($api_owner.status // "missing"),
                owner_missing: ($api_owner.owner_missing // ["owner-not-indexed"]),
                signature_count: ($signatures | length),
                verified_count: ([$signatures[] | select(.status == "verified")] | length),
                gap_count: ([$signatures[] | select(.status != "verified")] | length),
                signatures: ($signatures | map({id, symbol, signature, status}))
            },
            public_surface_reason: (if ($signatures | length) == 0 then "No callable signatures are indexed; the public surface remains unverified" else null end)
          })) as $owners
    | {
        format: "tondo-stdlib-implementation-coordination/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: (if all($owners[]; .implementation_status == "verified" and .matrix_impl_host == "verified" and .public_api.status == "verified") then "closed-coordination" else "open-gaps" end),
        sources: {
          implementation: "testing/stdlib-implementation.json",
          public_api: "testing/stdlib-public-api.json",
          normative_matrix: "testing/stdlib-matrix.json",
          owner_evidence: "testing/stdlib-owner-evidence.json",
          tracker: "TONDO_IMPLEMENTATION_TRACKER.md"
        },
        rules: {
          required_owner_status: "implemented-draft",
          implementation_stage_must_be_verified: true,
          callable_public_signatures_must_be_verified: true,
          empty_public_surface_is_unverified: true,
          global_public_audit_is_not_promoted: true,
          no_waivers: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          signatures: ([$owners[].public_api.signatures[]] | length),
          verified_signatures: ([$owners[].public_api.signatures[] | select(.status == "verified")] | length),
          owners_with_public_surface: ([$owners[] | select(.public_api.signature_count > 0)] | length),
          owners_without_callable_surface: ([$owners[] | select(.public_api.signature_count == 0)] | length),
          owners_with_public_gaps: ([$owners[] | select(.public_api.status != "verified")] | length)
        },
        global_public_api: {
          status: $api.status,
          gaps: $api.summary.gaps,
          reason: "The current global public API audit is retained independently of this component coordinator",
          next_coordination: "STD-PUBLIC-API-AUDIT-001"
        },
        next_coordination: "STD-B-INTEGRATION-PLAN-001"
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib implementation coordination: generated output must end with LF" >&2
    exit 1
}

echo "stdlib implementation coordination generated: $output"
