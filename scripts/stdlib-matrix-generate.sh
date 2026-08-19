#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-matrix.json}"
expanded_output="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-matrix-expanded.XXXXXX")"
trap 'rm -f "$expanded_output"' EXIT

for path in \
    testing/stdlib-implementation.json \
    testing/stdlib-meta.json \
    testing/stdlib-reflect.json \
    testing/stdlib-bytes.json \
    testing/stdlib-time.json \
    testing/stdlib-env.json \
    testing/stdlib-async.json \
    testing/stdlib-owner-evidence.json \
    testing/stdlib-public-api.json \
    testing/stdlib-spec.json \
    testing/stdlib-performance.json \
    testing/stdlib-performance-conformance.json \
    testing/stdlib-codec-conformance.json \
    testing/stdlib-core.json \
    testing/stdlib-hosted.json \
    testing/stdlib-serialization.json \
    testing/stdlib-json.json \
    testing/stdlib-messagepack.json \
    testing/stdlib-protobuf.json \
    testing/stdlib-testing.json; do
    [[ -f "$path" ]] || {
        echo "stdlib matrix: missing source $path" >&2
        exit 1
    }
done

mkdir -p "$(dirname "$output")"

jq -n \
    --slurpfile implementation testing/stdlib-implementation.json \
    --slurpfile meta_owner testing/stdlib-meta.json \
    --slurpfile reflect_owner testing/stdlib-reflect.json \
    --slurpfile bytes_owner testing/stdlib-bytes.json \
    --slurpfile time_owner testing/stdlib-time.json \
    --slurpfile env_owner testing/stdlib-env.json \
    --slurpfile async_owner testing/stdlib-async.json \
    --slurpfile owner_evidence testing/stdlib-owner-evidence.json \
    --slurpfile public_api testing/stdlib-public-api.json \
    --slurpfile integration testing/stdlib-spec.json \
    --slurpfile performance testing/stdlib-performance.json \
    --slurpfile performance_conformance testing/stdlib-performance-conformance.json \
    --slurpfile codec_conformance testing/stdlib-codec-conformance.json \
    --slurpfile core testing/stdlib-core.json \
    --slurpfile hosted testing/stdlib-hosted.json \
    --slurpfile serialization testing/stdlib-serialization.json \
    --slurpfile json_owner testing/stdlib-json.json \
    --slurpfile messagepack_owner testing/stdlib-messagepack.json \
    --slurpfile protobuf_owner testing/stdlib-protobuf.json \
    --slurpfile testing_owner testing/stdlib-testing.json \
    '
    def normalize_requirement:
        if type == "string" then
            {id: ., required: true, observables: []}
        else .
        end;

    def owner_ids($record):
        if (($record.owners? // null) | type) == "array" then
            $record.owners
        elif (($record.owner? // null) | type) == "string" then
            [$record.owner]
        else
            []
        end;

    def owner_contract($id; $sources):
        (first($sources[] | select((owner_ids(.data) | index($id)) != null)) // null);

    def implementation_owner($id; $manifest):
        (first($manifest.owners[] | select(.id == $id)) // null);

    def spec_owner($id; $integration):
        (first($integration.owner_contracts[] | select(.id == $id)) // null);

    def evidence_owner($id; $evidence):
        (first(($evidence[0].owners // [])[] | select(.id == $id)) // null);

    def evidence_stage($id; $stage; $fallback; $evidence):
        (evidence_owner($id; $evidence)) as $owner
        | if $owner == null then
            $fallback
          elif $stage == "IMPL/HOST" then
            ([$owner.cells.IMPL, $owner.cells.HOST] | map(select(. != null))) as $cells
            | if all($cells[]; .status == "verified" or .status == "not-applicable") then
                {status: "verified", reason: null, refs: ([$cells[] | .refs[]] | unique)}
              else
                {status: (if any($cells[]; .status == "gap") then "gap" elif any($cells[]; .status == "pending") then "pending" else "partial" end), reason: ([$cells[] | select(.reason != null) | .reason] | unique | join("; ")), refs: ([$cells[] | .refs[]] | unique)}
              end
          elif $stage == "MODEL/TEST/FUZZ" then
            ([$owner.cells.MODEL, $owner.cells.TEST, $owner.cells.FUZZ] | map(select(. != null))) as $cells
            | if all($cells[]; .status == "verified" or .status == "not-applicable") then
                {status: "verified", reason: null, refs: ([$cells[] | .refs[]] | unique)}
              else
                {status: (if any($cells[]; .status == "gap") then "gap" elif any($cells[]; .status == "pending") then "pending" else "partial" end), reason: ([$cells[] | select(.reason != null) | .reason] | unique | join("; ")), refs: ([$cells[] | .refs[]] | unique)}
              end
          elif ($owner.cells[$stage] // null) != null then
            $owner.cells[$stage]
          else
            $fallback
          end;

    def performance_owner($id; $manifest):
        (first($manifest.owners[] | select(.id == $id)) // null);

    def performance_group($id; $contract):
        (first($contract.owner_groups[] | select((.owners | index($id)) != null)) // null);

    def performance_stage($id; $contract; $manifest):
        (performance_owner($id; $manifest)) as $owner
        | if $owner == null then
            {status: "gap", reason: "owner is absent from the performance coordinator", refs: [], observed_dimensions: [], pending_dimensions: []}
          elif $owner.state == "captured-partial" then
            {status: "partial", reason: "only the dimensions listed as observed are captured; promotion remains pending", refs: ["testing/stdlib-performance-conformance.json#owners/" + $id], observed_dimensions: ($owner.dimensions // []), pending_dimensions: ($owner.pending_dimensions // [])}
          else
            {status: "pending", reason: $owner.reason, refs: ["testing/stdlib-performance-conformance.json#owners/" + $id], observed_dimensions: [], pending_dimensions: (($contract.dimensions | map(.id)) // [])}
          end;

    def conformance_stage($id; $codec):
        if (["std.serialization", "std.json", "std.messagepack", "std.protobuf"] | index($id)) != null then
            {status: "partial", reason: "the external codec harness is closed for the kernel and bridge, but the owner matrix still has public API gaps", refs: ["testing/stdlib-codec-conformance.json", "scripts/stdlib-codec-conformance.sh"]}
        else
            {status: "pending", reason: "STD-CONF-001 remains open until the owner has complete public evidence", refs: ["TONDO_IMPLEMENTATION_TRACKER.md#STD-CONF-001"]}
        end;

    def doc_stage($doc):
        if ($doc | type) == "string" and ($doc | length) > 0 then
            {status: "verified", reason: null, refs: [$doc]}
        else
            {status: "gap", reason: "owner has no normative contract document", refs: []}
        end;

    def implementation_stage($id; $manifest):
        (implementation_owner($id; $manifest)) as $owner
        | if $owner == null then
            {status: "partial", reason: "intrinsic owner is not indexed in testing/stdlib-implementation.json; STD-A-BYTES-EVIDENCE-001 remains open", refs: ["docs/contracts/stdlib-bytes.md", "TONDO_STANDARD_LIBRARY_SPEC.md"]}
          else
            {status: "verified", reason: null, refs: (($owner.implementation + $owner.tests) | unique)}
          end;

    def model_stage($id; $manifest; $has_contract_matrix):
        (implementation_owner($id; $manifest)) as $owner
        | if $owner == null then
            {status: "pending", reason: "no owner implementation record exists", refs: []}
          elif ($has_contract_matrix | not) then
            {status: "partial", reason: "the owner has implementation tests, but no per-owner executable test matrix is declared", refs: ($owner.tests | unique)}
          elif $id == "std.async" then
            {status: "partial", reason: "the implementation proof is historical kernel evidence; MODEL/TEST/FUZZ for the inferred ABI is pending", refs: ($owner.tests | unique)}
          else
            {status: "partial", reason: "owner tests exist; per-leaf MODEL/TEST/FUZZ links and fuzz coverage remain to be coordinated by STD-TEST-001", refs: ($owner.tests | unique)}
          end;

    def audit_implementation_stage($row):
        ($row.missing // []) as $missing
        | ($missing | map(select(. == "hir-symbol" or . == "lowering-symbol" or . == "host-symbol" or . == "vm-symbol"))) as $implementation_gaps
        | if ($implementation_gaps | length) == 0 then
            {status: "verified", reason: null, refs: ["testing/stdlib-public-api.json#rows/" + $row.id]}
          else
            {status: "partial", reason: ("public API audit implementation gaps: " + ($implementation_gaps | join(","))), refs: ["testing/stdlib-public-api.json#rows/" + $row.id]}
          end;

    def audit_model_stage($row; $owner_model):
        ($row.missing // []) as $missing
        | ($missing | map(select(. == "public-case-call" or . == "invalid-runtime-case" or . == "invalid-compile-case" or . == "invalid-runner-case"))) as $case_gaps
        | if ($case_gaps | length) == 0 and $owner_model.status == "verified" then
            {status: "partial", reason: "public case is present; per-leaf MODEL/TEST/FUZZ coordination is still open", refs: [$row.evidence.public_case.path]}
          elif ($case_gaps | length) == 0 then
            {status: "partial", reason: $owner_model.reason, refs: [$row.evidence.public_case.path]}
          else
            {status: "partial", reason: ("public-case gaps: " + ($case_gaps | join(","))), refs: [$row.evidence.public_case.path]}
          end;

    def row_status($stages):
        if all($stages[]; .status == "verified") then "verified" else "open-gaps" end;

    def performance_dimensions($id; $contract; $manifest):
        (performance_group($id; $contract)) as $group
        | (performance_stage($id; $contract; $manifest)) as $stage
        | {required: (($group.required_dimensions // []) | sort | unique), observed: ($stage.observed_dimensions | sort | unique), pending: ($stage.pending_dimensions | sort | unique)};

    def requirement_definitions($id; $sources):
        (owner_contract($id; $sources)) as $source
        | if $source == null then
            [{id: "owner-contract", required: true, observables: [], synthetic: true}]
          else
            (($source.data.test_matrix // []) | map(normalize_requirement))
          end;

    def source_for_owner($id; $sources):
        (owner_contract($id; $sources)) as $source
        | if $source == null then null else $source.path end;

    def owner_has_test_matrix($id; $sources):
        (owner_contract($id; $sources)) as $source
        | $source != null and (($source.data.test_matrix // []) | length) > 0;

    def contract_for_owner($id; $integration):
        (spec_owner($id; $integration)) as $owner
        | if $owner == null then
            (if $id == "std.bytes" then "docs/contracts/stdlib-bytes.md" else null end)
          else $owner.contract
          end;

    def owner_stage_templates($id; $manifest; $integration; $performance_contract; $performance_manifest; $codec; $sources; $evidence):
        (owner_contract($id; $sources)) as $source
        | (if $source == null then
            {status: "partial", reason: "no executable owner test_matrix entry exists; the placeholder makes the missing contract explicit", refs: [contract_for_owner($id; $integration)]}
          else
            {status: "verified", reason: null, refs: [source_for_owner($id; $sources), contract_for_owner($id; $integration)]}
          end) as $spec_stage
        | [{id: "SPEC", value: evidence_stage($id; "SPEC"; $spec_stage; $evidence)}, {id: "IMPL/HOST", value: evidence_stage($id; "IMPL/HOST"; implementation_stage($id; $manifest); $evidence)}, {id: "MODEL/TEST/FUZZ", value: evidence_stage($id; "MODEL/TEST/FUZZ"; model_stage($id; $manifest; owner_has_test_matrix($id; $sources)); $evidence)}, {id: "PERF", value: evidence_stage($id; "PERF"; performance_stage($id; $performance_contract; $performance_manifest); $evidence)}, {id: "CONF", value: evidence_stage($id; "CONF"; conformance_stage($id; $codec); $evidence)}, {id: "DOC", value: evidence_stage($id; "DOC"; doc_stage(contract_for_owner($id; $integration)); $evidence)}];

    def owner_rows($id; $manifest; $api; $integration; $performance_contract; $performance_manifest; $codec; $sources; $evidence):
        (spec_owner($id; $integration)) as $spec
        | (implementation_owner($id; $manifest)) as $implementation
        | (performance_dimensions($id; $performance_contract; $performance_manifest)) as $dimensions
        | (performance_stage($id; $performance_contract; $performance_manifest)) as $perf
        | (doc_stage(contract_for_owner($id; $integration))) as $doc
        | (implementation_stage($id; $manifest)) as $owner_impl
        | (requirement_definitions($id; $sources) | map(. as $requirement
            | (if ($requirement.synthetic // false) then
                {status: "partial", reason: "no executable owner test_matrix entry exists; the placeholder makes the missing contract explicit", refs: ([contract_for_owner($id; $integration)] + (if source_for_owner($id; $sources) == null then [] else [source_for_owner($id; $sources)] end))}
              else
                {status: "verified", reason: null, refs: [source_for_owner($id; $sources), contract_for_owner($id; $integration)]}
              end) as $spec_stage
            | (model_stage($id; $manifest; ($requirement.synthetic // false) | not)) as $model
            | [{id: "SPEC", value: evidence_stage($id; "SPEC"; $spec_stage; $evidence)}, {id: "IMPL/HOST", value: evidence_stage($id; "IMPL/HOST"; $owner_impl; $evidence)}, {id: "MODEL/TEST/FUZZ", value: evidence_stage($id; "MODEL/TEST/FUZZ"; $model; $evidence)}, {id: "PERF", value: evidence_stage($id; "PERF"; $perf; $evidence)}, {id: "CONF", value: evidence_stage($id; "CONF"; conformance_stage($id; $codec); $evidence)}, {id: "DOC", value: evidence_stage($id; "DOC"; $doc; $evidence)}] as $stages
            | {id: ("requirement:" + $id + ":" + $requirement.id), kind: "requirement", owner: $id, layer: ($implementation.layer // ($spec.source_set // "A0")), scope: "STD-0.1A", requirement: $requirement, source: {contract: contract_for_owner($id; $integration), owner_contract: source_for_owner($id; $sources)}, dimensions: $dimensions, stages: $stages, status: row_status($stages | map(.value))}
        )) as $requirement_rows
        | ($api.rows | map(select(.owner == $id)) | map(. as $row
            | (audit_implementation_stage($row)) as $impl_stage
            | (audit_model_stage($row; $owner_impl)) as $model_stage
            | [{id: "SPEC", value: evidence_stage($id; "SPEC"; {status: "verified", reason: null, refs: [$row.contract + "#" + ($row.line | tostring)]}; $evidence)}, {id: "IMPL/HOST", value: evidence_stage($id; "IMPL/HOST"; $impl_stage; $evidence)}, {id: "MODEL/TEST/FUZZ", value: evidence_stage($id; "MODEL/TEST/FUZZ"; $model_stage; $evidence)}, {id: "PERF", value: evidence_stage($id; "PERF"; $perf; $evidence)}, {id: "CONF", value: evidence_stage($id; "CONF"; conformance_stage($id; $codec); $evidence)}, {id: "DOC", value: evidence_stage($id; "DOC"; $doc; $evidence)}] as $stages
            | {id: ("signature:" + $row.id), kind: "signature", owner: $id, layer: ($implementation.layer // ($spec.source_set // "A0")), scope: "STD-0.1A", signature: $row.signature, symbol: $row.symbol, source: {contract: $row.contract, line: $row.line, audit: ("testing/stdlib-public-api.json#rows/" + $row.id)}, dimensions: $dimensions, stages: $stages, status: row_status($stages | map(.value))}
        )) as $signature_rows
        | ($requirement_rows + $signature_rows);

    ($implementation[0]) as $manifest
    | ($public_api[0]) as $api
    | ($integration[0]) as $integration_contract
    | ($performance[0]) as $performance_contract
    | ($performance_conformance[0]) as $performance_manifest
    | ($codec_conformance[0]) as $codec
    | ($owner_evidence) as $evidence
    | ([
        {path: "testing/stdlib-meta.json", data: $meta_owner[0]},
        {path: "testing/stdlib-reflect.json", data: $reflect_owner[0]},
        {path: "testing/stdlib-bytes.json", data: $bytes_owner[0]},
        {path: "testing/stdlib-time.json", data: $time_owner[0]},
        {path: "testing/stdlib-env.json", data: $env_owner[0]},
        {path: "testing/stdlib-async.json", data: $async_owner[0]},
        {path: "testing/stdlib-core.json", data: $core[0]},
        {path: "testing/stdlib-hosted.json", data: $hosted[0]},
        {path: "testing/stdlib-serialization.json", data: $serialization[0]},
        {path: "testing/stdlib-json.json", data: $json_owner[0]},
        {path: "testing/stdlib-messagepack.json", data: $messagepack_owner[0]},
        {path: "testing/stdlib-protobuf.json", data: $protobuf_owner[0]},
        {path: "testing/stdlib-testing.json", data: $testing_owner[0]}
    ]) as $sources
    | (($integration_contract.owner_contracts | map(.id)) + ["std.bytes"] | unique) as $owner_ids
    | ($owner_ids | map(. as $id | {id: $id, rows: owner_rows($id; $manifest; $api; $integration_contract; $performance_contract; $performance_manifest; $codec; $sources; $evidence)})) as $bundles
    | ($bundles | map(.rows[]) | sort_by([.owner, .kind, (.source.line // 0), (.requirement.id // ""), .id])) as $rows
    | ($bundles | map(.id as $id | (spec_owner($id; $integration_contract)) as $spec | (implementation_owner($id; $manifest)) as $implementation | (performance_dimensions($id; $performance_contract; $performance_manifest)) as $dimensions | {id: $id, layer: ($implementation.layer // "A0"), source_set: ($spec.source_set // "stdlib-core"), dependencies: ($spec.dependencies // []), contract: contract_for_owner($id; $integration_contract), owner_contract: source_for_owner($id; $sources), implementation_indexed: ($implementation != null), dimensions: $dimensions, stages: (owner_stage_templates($id; $manifest; $integration_contract; $performance_contract; $performance_manifest; $codec; $sources; $evidence) | reduce .[] as $stage ({}; .[$stage.id] = $stage.value)), signature_rows: ([ $rows[] | select(.owner == $id and .kind == "signature") | .id ]), requirement_rows: ([ $rows[] | select(.owner == $id and .kind == "requirement") | .id ]), status: (if any($rows[]; .owner == $id and .status == "open-gaps") then "open-gaps" else "verified" end)})) as $owners
    | {
        format: "tondo-stdlib-normative-matrix/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: (if any($rows[]; .status == "open-gaps") then "open-gaps" else "verified" end),
        catalogs: {current: "STD-0.1A", future_closed: "STD-0.1B", future_modules: ["std.encoding", "std.yaml", "std.toml", "std.cbor", "std.regex", "std.uuid", "std.channel", "std.sync", "std.executor", "std.log", "std.net"]},
        sources: {canonical_spec: "TONDO_STANDARD_LIBRARY_SPEC.md", integration: "testing/stdlib-spec.json", implementation: "testing/stdlib-implementation.json", owner_contracts: ["testing/stdlib-meta.json", "testing/stdlib-reflect.json", "testing/stdlib-bytes.json", "testing/stdlib-time.json", "testing/stdlib-env.json", "testing/stdlib-async.json"], owner_evidence: "testing/stdlib-owner-evidence.json", public_api: "testing/stdlib-public-api.json", performance_contract: "testing/stdlib-performance.json", performance_coordinator: "testing/stdlib-performance-conformance.json", codec_conformance: "testing/stdlib-codec-conformance.json"},
        rules: {required_stages: ["SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC"], one_owner_per_signature: true, one_owner_per_requirement: true, pending_requires_reason: true, not_applicable_requires_reason: true, future_catalog_not_implicitly_current: true},
        owners: $owners,
        rows: $rows,
        summary: {owners: ($owners | length), signatures: ($rows | map(select(.kind == "signature")) | length), requirements: ($rows | map(select(.kind == "requirement")) | length), rows: ($rows | length), verified_rows: ($rows | map(select(.status == "verified")) | length), open_rows: ($rows | map(select(.status == "open-gaps")) | length), stage_counts: (reduce $rows[] as $row ({}; reduce $row.stages[] as $stage (. ; .[$stage.id] = ((.[$stage.id] // {}) | .[$stage.value.status] = ((.[$stage.value.status] // 0) + 1)))))}
    }
    ' > "$expanded_output"

jq -S '
    . as $matrix
    | {
        format: $matrix.format,
        edition: $matrix.edition,
        phase: $matrix.phase,
        status: $matrix.status,
        catalogs: $matrix.catalogs,
        sources: $matrix.sources,
        rules: ($matrix.rules + {stage_refs_are_explicit: true, dimensions_are_owner_scoped: true}),
        owners: ($matrix.owners | map(.stages = (.stages // {}) | . + {stage_order: ["SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC"]})),
        rows: ($matrix.rows | map(
            . as $row
            | {
                id: $row.id,
                kind: $row.kind,
                owner: $row.owner,
                layer: $row.layer,
                scope: $row.scope,
                signature: ($row.signature // null),
                symbol: ($row.symbol // null),
                requirement: ($row.requirement // null),
                source: $row.source,
                dimensions_ref: ("owner:" + $row.owner),
                stage_refs: (if $row.kind == "signature" then
                    {
                        "SPEC": ("audit:" + $row.source.audit + ":SPEC"),
                        "IMPL/HOST": ("audit:" + $row.source.audit + ":IMPL/HOST"),
                        "MODEL/TEST/FUZZ": ("audit:" + $row.source.audit + ":MODEL/TEST/FUZZ"),
                        "PERF": ("owner:" + $row.owner + ":PERF"),
                        "CONF": ("owner:" + $row.owner + ":CONF"),
                        "DOC": ("owner:" + $row.owner + ":DOC")
                    }
                  else
                    {
                        "SPEC": ("owner:" + $row.owner + ":SPEC"),
                        "IMPL/HOST": ("owner:" + $row.owner + ":IMPL/HOST"),
                        "MODEL/TEST/FUZZ": ("owner:" + $row.owner + ":MODEL/TEST/FUZZ"),
                        "PERF": ("owner:" + $row.owner + ":PERF"),
                        "CONF": ("owner:" + $row.owner + ":CONF"),
                        "DOC": ("owner:" + $row.owner + ":DOC")
                    }
                  end),
                status: $row.status
            }
            | with_entries(select(.value != null))
        )),
        summary: $matrix.summary
      }
    ' "$expanded_output" > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib matrix: generated output must end with LF" >&2
    exit 1
}

echo "stdlib matrix generated: $output"
