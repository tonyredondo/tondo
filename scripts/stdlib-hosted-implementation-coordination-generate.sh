#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-hosted-implementation-coordination.json}"
mkdir -p "$(dirname "$output")"

# STD-IMPL-002 coordinates the four hosted owners as one vertical slice.  The
# owner contract remains the source of capability truth; the implementation
# manifest, public audit, matrix and per-owner evidence must all agree before
# the coordinator can close.
jq -n \
    --slurpfile contract testing/stdlib-hosted.json \
    --slurpfile implementation testing/stdlib-implementation.json \
    --slurpfile api testing/stdlib-public-api.json \
    --slurpfile matrix testing/stdlib-matrix.json \
    --slurpfile evidence testing/stdlib-owner-evidence.json \
    '
    ($contract[0]) as $contract
    | ($implementation[0]) as $implementation
    | ($api[0]) as $api
    | ($matrix[0]) as $matrix
    | ($evidence[0]) as $evidence
    | ["std.console", "std.path", "std.fs", "std.process"] as $required
    | ($required | map(. as $id
        | (first($implementation.owners[] | select(.id == $id))) as $owner
        | (first($matrix.owners[] | select(.id == $id))) as $matrix_owner
        | (first($evidence.owners[] | select(.id == $id))) as $evidence_owner
        | ([ $api.rows[] | select(.owner == $id) ] | sort_by(.line)) as $signatures
        | {
            id: $id,
            layer: $owner.layer,
            capability: $contract.capabilities[$id],
            implementation: $owner.implementation,
            tests: $owner.tests,
            proof: $owner.proof,
            implementation_status: (if ($owner.implementation | length) > 0 and ($owner.tests | length) > 0 and $evidence_owner.cells.IMPL.status == "verified" then "verified" else "open-gaps" end),
            matrix_impl_host: $matrix_owner.stages["IMPL/HOST"].status,
            host: {
              status: $evidence_owner.cells.HOST.status,
              reason: $evidence_owner.cells.HOST.reason,
              refs: $evidence_owner.cells.HOST.refs
            },
            public_api: {
                status: (if all($signatures[]; .status == "verified") then "verified" else "open-gaps" end),
                signature_count: ($signatures | length),
                verified_count: ([$signatures[] | select(.status == "verified")] | length),
                gap_count: ([$signatures[] | select(.status != "verified")] | length),
                signatures: ($signatures | map({id, symbol, signature, status}))
            }
          })) as $owners
    | {
        format: "tondo-stdlib-hosted-implementation-coordination/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "closed-coordination",
        sources: {
          owner_contract: "testing/stdlib-hosted.json",
          implementation: "testing/stdlib-implementation.json",
          public_api: "testing/stdlib-public-api.json",
          normative_matrix: "testing/stdlib-matrix.json",
          owner_evidence: "testing/stdlib-owner-evidence.json",
          tracker: "TONDO_IMPLEMENTATION_TRACKER.md"
        },
        rules: {
          required_owner_status: "implemented-draft",
          implementation_stage_must_be_verified: true,
          hosted_stage_must_be_verified_or_not_applicable: true,
          capability_boundary_must_match_contract: true,
          callable_public_signatures_must_be_verified: true,
          global_public_audit_is_not_promoted: true,
          no_waivers: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          signatures: ([$owners[].public_api.signatures[]] | length),
          verified_signatures: ([$owners[].public_api.signatures[] | select(.status == "verified")] | length),
          capability_gated_owners: ([$owners[] | select((.capability | length) > 0)] | length),
          pure_owners: ([$owners[] | select((.capability | length) == 0)] | length),
          host_verified_owners: ([$owners[] | select(.host.status == "verified")] | length),
          host_not_applicable_owners: ([$owners[] | select(.host.status == "not-applicable")] | length)
        },
        global_public_api: {
          status: $api.status,
          gaps: $api.summary.gaps,
          reason: "STD-CODEC-PUBLIC-001 closed MessagePack/Protobuf callable exposure and indexed the build-only owners without a runtime waiver",
          next_coordination: "NATIVE-TARGET-DESC-001"
        },
        next_coordination: "NATIVE-TARGET-DESC-001"
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib hosted implementation coordination: generated output must end with LF" >&2
    exit 1
}

echo "stdlib hosted implementation coordination generated: $output"
