#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-conformance-coordination.json}"
mkdir -p "$(dirname "$output")"

jq -n \
    --slurpfile matrix testing/stdlib-matrix.json \
    --slurpfile evidence testing/stdlib-owner-evidence.json \
    --slurpfile api testing/stdlib-public-api.json \
    --slurpfile testing_coordination testing/stdlib-test-coordination.json \
    --slurpfile public_conformance testing/stdlib-conformance.json \
    '
    def codec_owners: ["std.serialization", "std.json", "std.messagepack", "std.protobuf"];

    ($matrix[0]) as $m
    | ($evidence[0]) as $e
    | ($api[0]) as $a
    | ($testing_coordination[0]) as $tc
    | ($public_conformance[0]) as $pc
    | ($m.owners | map(.id) | sort) as $owner_ids
    | ($m.rows | sort_by(.id)) as $matrix_rows
    | ($owner_ids | map(. as $id
        | ([ $matrix_rows[] | select(.owner == $id) ] | sort_by(.id)) as $rows
        | (first($m.owners[] | select(.id == $id))) as $matrix_owner
        | $matrix_owner.stages.CONF as $matrix_conf
        | (first($e.owners[] | select(.id == $id)) // null) as $evidence_owner
        | (first($pc.owners[] | select(.id == $id))) as $public_owner
        | $matrix_conf.status as $status
        | {
            id: $id,
            layer: (first($m.owners[] | select(.id == $id)).layer),
            status: $status,
            reason: $matrix_conf.reason,
            public_signatures: ([ $a.rows[] | select(.owner == $id) | .id ] | sort),
            requirements: ([ $rows[] | select(.kind == "requirement") | .id ] | sort),
            rows: ([ $rows[] | {
              id,
              kind,
              status: $matrix_conf.status,
              reason: $matrix_conf.reason,
              refs: $matrix_conf.refs
            } ] | sort_by(.id)),
            evidence: {
              status: $status,
              refs: ((($matrix_conf.refs // []) + (($evidence_owner.cells.CONF.refs // []) | unique) + ($public_owner.refs // []) + ["testing/stdlib-matrix.json"] + (if ([$rows[].kind] | index("signature")) != null then ["testing/stdlib-public-api.json"] else [] end)) | unique | sort),
              commands: (((($evidence_owner.commands // []) + ["scripts/stdlib-matrix-check.sh", "scripts/stdlib-test-coordination-check.sh"] + (if (codec_owners | index($id)) != null then ["scripts/stdlib-codec-conformance.sh"] else [] end)) | unique | sort)),
              cases: ($public_owner.cases | map(.id) | sort),
              scope: (if $evidence_owner == null then "synthetic-owner-gap" else $public_owner.scope end)
            }
          }) | sort_by(.id)) as $owners
    | ($owners | map(.rows[]) ) as $rows
    | {
        format: "tondo-stdlib-conformance-coordination/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "promoted",
        sources: {
          normative_matrix: "testing/stdlib-matrix.json",
          owner_evidence: "testing/stdlib-owner-evidence.json",
          public_api: "testing/stdlib-public-api.json",
          model_test_coordination: "testing/stdlib-test-coordination.json",
          public_conformance: "testing/stdlib-conformance.json",
          codec_conformance: "testing/stdlib-codec-conformance.json",
          conformance_contract: "docs/contracts/conformance.md"
        },
        runner: {
          lineage: "draft",
          validate_command: "cargo run -p tondo-conformance --locked -- validate --root . --manifest conformance/draft/manifest.json --lineage draft",
          run_command: "cargo run -p tondo-conformance --locked -- run --root . --manifest conformance/draft/manifest.json --lineage draft --adapter target/debug/tondo-reference-adapter --evidence target/reliability/evidence/layer-evidence.json --output target/reliability/evidence/conformance-result.json",
          result: "target/reliability/evidence/conformance-result.json"
        },
        rules: {
          one_owner_per_matrix_row: true,
          every_matrix_row_has_conf_record: true,
          pending_requires_reason: true,
          partial_requires_reason: true,
          refs_are_explicit: true,
          verified_requires_observation: true,
          coordination_does_not_promote: false,
          execution_registry: "testing/stdlib-conformance.json"
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          rows: ($rows | length),
          public_signatures: ([$rows[] | select(.kind == "signature")] | length),
          requirements: ([$rows[] | select(.kind == "requirement")] | length),
          verified_rows: ([$rows[] | select(.status == "verified")] | length),
          partial_rows: ([$rows[] | select(.status == "partial")] | length),
          pending_rows: ([$rows[] | select(.status == "pending")] | length),
          owner_verified: ([$owners[] | select(.status == "verified")] | length),
          owner_partial: ([$owners[] | select(.status == "partial")] | length),
          owner_pending: ([$owners[] | select(.status == "pending")] | length)
        },
        promotion: {
          status: "promoted",
          reason: "STD-A-CONF-001 executed every owner command and runtime sidecar, plus the complete 206-case draft suite",
          matrix_status: $m.status,
          next_coordination: "STD-A-DIST-001"
        }
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib conformance coordination: generated output must end with LF" >&2
    exit 1
}

echo "stdlib conformance coordination generated: $output"
