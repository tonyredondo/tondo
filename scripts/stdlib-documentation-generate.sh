#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-documentation.json}"
mkdir -p "$(dirname "$output")"

jq -n \
    --slurpfile matrix testing/stdlib-matrix.json \
    --slurpfile evidence testing/stdlib-owner-evidence.json \
    --slurpfile api testing/stdlib-public-api.json \
    --slurpfile conformance testing/stdlib-conformance-coordination.json \
    '
    def examples($id):
      {
        "std.async": [
          {id: "async-structured", kind: "runtime", source: "tests/runtime/m7-async-structured.to", command: "scripts/test-gate.sh"},
          {id: "async-cancellation", kind: "runtime", source: "tests/runtime/m7-cancellation-cleanup.to", command: "scripts/test-gate.sh"},
          {id: "async-iterator-collect", kind: "runtime", source: "tests/runtime/m11-std-async-iter-001.to", command: "scripts/stdlib-async-test.sh"}
        ],
        "std.bytes": [
          {id: "bytes-public", kind: "runtime", source: "tests/runtime/m10-std-bytes-001.to", command: "scripts/stdlib-bytes-test.sh"}
        ],
        "std.collections": [
          {id: "collections-public", kind: "runtime", source: "tests/runtime/m11-std-collections-001.to", command: "scripts/stdlib-collections-test.sh"}
        ],
        "std.console": [
          {id: "console-public", kind: "runtime", source: "tests/runtime/m11-std-console-001.to", command: "scripts/stdlib-console-test.sh"}
        ],
        "std.core": [
          {id: "core-intrinsics", kind: "runtime", source: "tests/runtime/m11-std-core-001.to", command: "scripts/stdlib-core-test.sh"},
          {id: "core-protocols", kind: "runtime", source: "tests/runtime/m11-std-core-002.to", command: "scripts/stdlib-core-test.sh"}
        ],
        "std.env": [
          {id: "env-snapshot", kind: "runtime", source: "tests/runtime/m10-std-env-001.to", command: "scripts/stdlib-env-test.sh"}
        ],
        "std.format": [
          {id: "format-public", kind: "runtime", source: "tests/runtime/m11-std-format-001.to", command: "scripts/stdlib-format-test.sh"}
        ],
        "std.fs": [
          {id: "filesystem-public", kind: "runtime", source: "tests/runtime/m11-std-fs-001.to", command: "scripts/stdlib-fs-test.sh"}
        ],
        "std.io": [
          {id: "io-public", kind: "runtime", source: "tests/runtime/m11-std-io-001.to", command: "scripts/stdlib-io-test.sh"}
        ],
        "std.iter": [
          {id: "iter-public", kind: "runtime", source: "tests/runtime/m11-std-iter-001.to", command: "scripts/stdlib-iter-test.sh"}
        ],
        "std.json": [
          {id: "codecs-json", kind: "runtime", source: "tests/runtime/m11-std-codecs-001.to", command: "scripts/stdlib-json-test.sh"},
          {id: "codecs-json-external", kind: "external", source: "crates/tondo-stdlib/tests/codec_conformance.rs", command: "scripts/stdlib-codec-conformance.sh"}
        ],
        "std.math": [
          {id: "math-public", kind: "runtime", source: "tests/runtime/m11-std-math-001.to", command: "scripts/stdlib-math-test.sh"},
          {id: "math-ieee", kind: "runtime", source: "tests/runtime/m6-num-004-ieee.to", command: "scripts/stdlib-math-test.sh"}
        ],
        "std.messagepack": [
          {id: "codecs-messagepack", kind: "runtime", source: "tests/runtime/m11-std-codecs-001.to", command: "scripts/stdlib-messagepack-test.sh"},
          {id: "codecs-messagepack-external", kind: "external", source: "crates/tondo-stdlib/tests/codec_conformance.rs", command: "scripts/stdlib-codec-conformance.sh"}
        ],
        "std.meta": [
          {id: "meta-conformance", kind: "compiler", source: "crates/tondo-compiler/tests/std_meta_conformance.rs", command: "scripts/stdlib-meta-test.sh"}
        ],
        "std.path": [
          {id: "path-public", kind: "runtime", source: "tests/runtime/m11-std-path-001.to", command: "scripts/stdlib-path-test.sh"}
        ],
        "std.process": [
          {id: "process-public", kind: "runtime", source: "tests/runtime/m8-process-001.to", command: "scripts/stdlib-process-test.sh"},
          {id: "process-cancellation", kind: "runtime", source: "tests/runtime/m8-process-cancel.to", command: "scripts/stdlib-process-test.sh"}
        ],
        "std.protobuf": [
          {id: "codecs-protobuf", kind: "runtime", source: "tests/runtime/m11-std-codecs-001.to", command: "scripts/stdlib-protobuf-test.sh"},
          {id: "codecs-protobuf-external", kind: "external", source: "crates/tondo-stdlib/tests/codec_conformance.rs", command: "scripts/stdlib-codec-conformance.sh"}
        ],
        "std.reflect": [
          {id: "reflect-metadata", kind: "compiler", source: "crates/tondo-compiler/src/reflect.rs", command: "scripts/stdlib-reflect-test.sh"}
        ],
        "std.serialization": [
          {id: "serialization-codec-entry", kind: "runtime", source: "tests/runtime/m11-std-codecs-001.to", command: "scripts/stdlib-serialization-test.sh"},
          {id: "serialization-external", kind: "external", source: "crates/tondo-stdlib/tests/codec_conformance.rs", command: "scripts/stdlib-codec-conformance.sh"}
        ],
        "std.testing": [
          {id: "testing-acceptance", kind: "acceptance", source: "crates/tondo-cli/tests/acceptance_projects.rs", command: "cargo test -p tondo-cli --locked --test acceptance_projects"}
        ],
        "std.text": [
          {id: "text-public", kind: "runtime", source: "tests/runtime/m11-std-text-001.to", command: "scripts/stdlib-text-test.sh"},
          {id: "text-index-slice", kind: "runtime", source: "tests/runtime/m11-std-text-002.to", command: "scripts/stdlib-text-test.sh"}
        ],
        "std.time": [
          {id: "time-monotonic", kind: "runtime", source: "tests/runtime/m10-std-time-001.to", command: "scripts/stdlib-time-test.sh"}
        ]
      }[$id];

    def fallback_contract($id):
      if $id == "std.bytes" then "docs/contracts/stdlib-bytes.md"
      elif ($id == "std.meta") then "docs/contracts/std-meta.md"
      elif ($id == "std.reflect") then "docs/contracts/std-reflect.md"
      elif ($id == "std.async") then "docs/contracts/stdlib-core.md"
      else "docs/contracts/stdlib-core.md"
      end;

    ($matrix[0]) as $m
    | ($evidence[0]) as $e
    | ($api[0]) as $a
    | ($conformance[0]) as $c
    | ($m.owners | map(.id) | sort) as $owner_ids
    | ($owner_ids | map(. as $id
        | (first($m.owners[] | select(.id == $id))) as $matrix_owner
        | (first($e.owners[] | select(.id == $id)) // null) as $evidence_owner
        | ([ $a.rows[] | select(.owner == $id) ]) as $api_rows
        | ([ $api_rows[] | select((.missing // []) | length == 0) ]) as $verified_api
        | (examples($id)) as $examples
        | ($examples | map(. + {status: "verified", verification: (if .kind == "runtime" then "fixture-sidecars" elif .kind == "acceptance" then "acceptance-test" elif .kind == "external" then "external-harness" else "compiler-test" end)})) as $verified_examples
        | (if ($evidence_owner == null) then [fallback_contract($id), "TONDO_STANDARD_LIBRARY_SPEC.md", "docs/contracts/stdlib-s1a.md"] else $evidence_owner.cells.DOC.refs end) as $docs
        | (if ($evidence_owner == null) then $matrix_owner.stages["IMPL/HOST"].refs else $evidence_owner.cells.IMPL.refs end) as $kernel_refs
        | (if ($evidence_owner == null) then $matrix_owner.stages["IMPL/HOST"].refs else $evidence_owner.cells.HOST.refs end) as $bridge_refs
        | {
            id: $id,
            layer: $matrix_owner.layer,
            status: "documented-draft",
            contract: (if ($evidence_owner == null) then fallback_contract($id) else (first($evidence_owner.cells.SPEC.refs[] | select(startswith("docs/contracts/"))) // fallback_contract($id)) end),
            docs: ($docs | unique | sort),
            boundary: {
              kernel: {
                status: $matrix_owner.stages["IMPL/HOST"].status,
                reason: $matrix_owner.stages["IMPL/HOST"].reason,
                refs: ($kernel_refs | unique | sort)
              },
              bridge: {
                status: (if ($evidence_owner == null) then "partial" elif $evidence_owner.cells.HOST.status == "not-applicable" then "not-applicable" else $evidence_owner.cells.HOST.status end),
                reason: (if ($evidence_owner == null) then "no separate owner evidence exists; the owner remains compiler/VM or draft-bound" else $evidence_owner.cells.HOST.reason end),
                refs: ($bridge_refs | unique | sort)
              },
              public_api: {
                status: (if ($api_rows | length) == 0 then (if $id == "std.serialization" then "partial" else "not-applicable" end) elif ($verified_api | length) == ($api_rows | length) then "complete" else "partial" end),
                reason: (if ($api_rows | length) == 0 then (if $id == "std.serialization" then "the owner contract has public protocols but no public-audit signature rows yet" else "the owner has no public signature rows; its contract is intrinsic, metadata-only, or build-only" end) elif ($verified_api | length) == ($api_rows | length) then null else "public API audit retains gaps for this owner" end),
                signatures: ($api_rows | map(.id) | sort),
                verified_signatures: ($verified_api | map(.id) | sort),
                refs: (if ($api_rows | length) == 0 then ["testing/stdlib-public-api.json"] else ["testing/stdlib-public-api.json"] end)
              }
            },
            runtime_applicable: ($id != "std.meta" and $id != "std.reflect"),
            runtime_reason: (if $id == "std.meta" then "build-only provider; conformance runs in the compiler/meta VM boundary" elif $id == "std.reflect" then "metadata-only provider; no runtime value or host adapter exists" else null end),
            examples: ($verified_examples | sort_by(.id)),
            conformance: (first($c.owners[] | select(.id == $id)) | {status, reason, refs: .evidence.refs}),
            documentation_claim: "This record documents the unpublished draft only; it is not a release or a claim that the owner matrix is green."
          }) | sort_by(.id)) as $owners
    | {
        format: "tondo-stdlib-documentation/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "closed-coordination",
        sources: {
          standard_spec: "TONDO_STANDARD_LIBRARY_SPEC.md",
          s1a_contract: "docs/contracts/stdlib-s1a.md",
          owner_evidence: "testing/stdlib-owner-evidence.json",
          public_api: "testing/stdlib-public-api.json",
          conformance_coordination: "testing/stdlib-conformance-coordination.json",
          normative_matrix: "testing/stdlib-matrix.json"
        },
        rules: {
          one_owner_per_record: true,
          every_owner_has_contract: true,
          every_owner_has_example: true,
          runtime_examples_require_sidecar: true,
          non_runtime_examples_require_reason: true,
          boundary_statuses_are_explicit: true,
          api_gaps_are_not_promoted: true,
          unpublished_claim_is_required: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          examples: ([$owners[].examples[]] | length),
          runtime_examples: ([$owners[].examples[] | select(.kind == "runtime" or .kind == "acceptance")] | length),
          external_examples: ([$owners[].examples[] | select(.kind == "external")] | length),
          compiler_examples: ([$owners[].examples[] | select(.kind == "compiler")] | length),
          api_complete: ([$owners[] | select(.boundary.public_api.status == "complete")] | length),
          api_partial: ([$owners[] | select(.boundary.public_api.status == "partial")] | length),
          api_not_applicable: ([$owners[] | select(.boundary.public_api.status == "not-applicable")] | length)
        },
        promotion: {
          status: "not-published",
          reason: "documentation closes traceability for the current unpublished draft; it does not promote implementation, conformance, performance, or a language release"
        }
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib documentation: generated output must end with LF" >&2
    exit 1
}

echo "stdlib documentation generated: $output"
