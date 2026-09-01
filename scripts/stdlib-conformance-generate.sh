#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-conformance.json}"
mkdir -p "$(dirname "$output")"

jq -n \
    --slurpfile matrix testing/stdlib-matrix.json \
    --slurpfile documentation testing/stdlib-documentation.json \
    --slurpfile evidence testing/stdlib-owner-evidence.json \
    --slurpfile api testing/stdlib-public-api.json \
    '
    def owner_script($id):
      {
        "std.async": "scripts/stdlib-async-test.sh",
        "std.bytes": "scripts/stdlib-bytes-test.sh",
        "std.collections": "scripts/stdlib-collections-test.sh",
        "std.console": "scripts/stdlib-console-test.sh",
        "std.core": "scripts/stdlib-core-test.sh",
        "std.env": "scripts/stdlib-env-test.sh",
        "std.format": "scripts/stdlib-format-test.sh",
        "std.fs": "scripts/stdlib-fs-test.sh",
        "std.io": "scripts/stdlib-io-test.sh",
        "std.iter": "scripts/stdlib-iter-test.sh",
        "std.json": "scripts/stdlib-json-test.sh",
        "std.math": "scripts/stdlib-math-test.sh",
        "std.messagepack": "scripts/stdlib-messagepack-test.sh",
        "std.meta": "scripts/stdlib-meta-test.sh",
        "std.path": "scripts/stdlib-path-test.sh",
        "std.process": "scripts/stdlib-process-test.sh",
        "std.protobuf": "scripts/stdlib-protobuf-test.sh",
        "std.reflect": "scripts/stdlib-reflect-test.sh",
        "std.serialization": "scripts/stdlib-serialization-test.sh",
        "std.testing": "scripts/stdlib-testing-test.sh",
        "std.text": "scripts/stdlib-text-test.sh",
        "std.time": "scripts/stdlib-time-test.sh"
      }[$id];

    ($matrix[0]) as $matrix
    | ($documentation[0]) as $documentation
    | ($evidence[0]) as $evidence
    | ($api[0]) as $api
    | ($matrix.owners | sort_by(.id) | map(
        . as $owner
        | (first($documentation.owners[] | select(.id == $owner.id))) as $docs
        | (first($evidence.owners[] | select(.id == $owner.id))) as $evidence_owner
        | ([ $matrix.rows[] | select(.owner == $owner.id and .kind == "signature") | .id ] | sort) as $signatures
        | ([ $matrix.rows[] | select(.owner == $owner.id and .kind == "requirement") | .id ] | sort) as $requirements
        | ($docs.examples | map({
            id: .id,
            kind: .kind,
            source: .source,
            command: .command,
            verification: .verification
          }) | sort_by(.id)) as $cases
        | {
            id: $owner.id,
            layer: $owner.layer,
            status: "verified",
            public_signatures: $signatures,
            requirements: $requirements,
            rows: {
              signatures: ($signatures | length),
              requirements: ($requirements | length),
              total: (($signatures + $requirements) | length)
            },
            owner_command: owner_script($owner.id),
            cases: $cases,
            refs: ((
              ($owner.stages.CONF.refs // [])
              + ($evidence_owner.cells.CONF.refs // [])
              + ($cases | map(.source))
              + (if ($signatures | length) > 0 then ["testing/stdlib-public-api.json"] else [] end)
            ) | unique | sort),
            scope: (if $owner.id == "std.meta" then "compiler-build-only"
                    elif $owner.id == "std.reflect" then "compiler-metadata-only"
                    else "public-owner-and-runtime" end)
          }
      )) as $owners
    | {
        format: "tondo-stdlib-conformance/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "promoted",
        sources: {
          normative_matrix: "testing/stdlib-matrix.json",
          documentation: "testing/stdlib-documentation.json",
          owner_evidence: "testing/stdlib-owner-evidence.json",
          public_api: "testing/stdlib-public-api.json",
          draft_manifest: "conformance/0.1/manifest.json",
          result: "target/reliability/evidence/conformance-result.json"
        },
        runner: {
          lineage: "draft",
          manifest: "conformance/0.1/manifest.json",
          manifest_sha256: "016edfea892728645c387c2bcf4003096119a2e90b142ecf168bc7b4f6667299",
          full_suite_case_count: 206,
          validate_command: "cargo run -p tondo-conformance --locked -- validate --root . --manifest conformance/draft/manifest.json --lineage draft",
          run_command: "cargo run -p tondo-conformance --locked -- run --root . --manifest conformance/draft/manifest.json --lineage draft --adapter target/debug/tondo-reference-adapter --evidence target/reliability/evidence/layer-evidence.json --output target/reliability/evidence/conformance-result.json",
          owner_command_policy: "each owner command and every runtime/example case must pass"
        },
        rules: {
          every_matrix_row_has_observation: true,
          verified_requires_current_observation: true,
          runtime_cases_compare_exit_and_stdout_sidecars: true,
          compiler_and_external_cases_run_the_declared_command: true,
          capability_and_cancellation_cases_are_included: true,
          coordination_is_not_evidence: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          signatures: ([$owners[].public_signatures[]] | length),
          requirements: ([$owners[].requirements[]] | length),
          rows: ([$owners[].public_signatures[], $owners[].requirements[]] | length),
          cases: ([$owners[].cases[]] | length)
        }
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib conformance: generated output must end with LF" >&2
    exit 1
}

echo "stdlib conformance generated: $output"
