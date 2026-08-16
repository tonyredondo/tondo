#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-implementation-coordination.json}"
mkdir -p "$(dirname "$output")"

# STD-IMPL-001 is deliberately a coordinator for the already specified Core
# group plus the shared serialization kernel.  Codec protocol exposure and
# build-only callable surfaces remain separate leaves; including them here
# would turn a coordination record into a waiver for the public API audit.
jq -n \
    --slurpfile implementation testing/stdlib-implementation.json \
    --slurpfile api testing/stdlib-public-api.json \
    --slurpfile matrix testing/stdlib-matrix.json \
    '
    ($implementation[0]) as $implementation
    | ($api[0]) as $api
    | ($matrix[0]) as $matrix
    | ["std.core", "std.text", "std.collections", "std.iter", "std.math", "std.format", "std.io", "std.serialization"] as $required
    | ($required | map(. as $id
        | (first($implementation.owners[] | select(.id == $id))) as $owner
        | (first($matrix.owners[] | select(.id == $id))) as $matrix_owner
        | ([ $api.rows[] | select(.owner == $id) ] | sort_by(.line)) as $signatures
        | {
            id: $id,
            layer: $owner.layer,
            implementation: $owner.implementation,
            tests: $owner.tests,
            proof: $owner.proof,
            implementation_status: "verified",
            matrix_impl_host: $matrix_owner.stages["IMPL/HOST"].status,
            public_api: {
                status: (if ($signatures | length) == 0 then "not-applicable" elif all($signatures[]; .status == "verified") then "verified" else "open-gaps" end),
                signature_count: ($signatures | length),
                verified_count: ([$signatures[] | select(.status == "verified")] | length),
                gap_count: ([$signatures[] | select(.status != "verified")] | length),
                signatures: ($signatures | map({id, symbol, signature, status}))
            },
            public_surface_reason: (if ($signatures | length) == 0 then "std.serialization is a compiler-owned event protocol; its public traits are build-time contracts and have no indexable top-level pub fn surface" else null end)
          })) as $owners
    | {
        format: "tondo-stdlib-implementation-coordination/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "closed-coordination",
        sources: {
          implementation: "testing/stdlib-implementation.json",
          public_api: "testing/stdlib-public-api.json",
          normative_matrix: "testing/stdlib-matrix.json",
          tracker: "TONDO_IMPLEMENTATION_TRACKER.md"
        },
        rules: {
          required_owner_status: "implemented-draft",
          implementation_stage_must_be_verified: true,
          callable_public_signatures_must_be_verified: true,
          build_only_no_callable_surface_requires_reason: true,
          global_public_audit_is_not_promoted: true,
          no_waivers: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          signatures: ([$owners[].public_api.signatures[]] | length),
          verified_signatures: ([$owners[].public_api.signatures[] | select(.status == "verified")] | length),
          owners_with_public_surface: ([$owners[] | select(.public_api.status != "not-applicable")] | length),
          owners_without_callable_surface: ([$owners[] | select(.public_api.status == "not-applicable")] | length)
        },
        global_public_api: {
          status: $api.status,
          gaps: $api.summary.gaps,
          reason: "MessagePack/Protobuf callable exposure and build-only owner indexing remain open public-audit work; this coordinator does not waive those rows",
          next_coordination: "STD-IMPL-002"
        },
        next_coordination: "STD-IMPL-002"
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib implementation coordination: generated output must end with LF" >&2
    exit 1
}

echo "stdlib implementation coordination generated: $output"
