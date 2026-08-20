#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

output="${1:-testing/stdlib-test-coordination.json}"
mkdir -p "$(dirname "$output")"

jq -n \
    --slurpfile evidence testing/stdlib-owner-evidence.json \
    --slurpfile api testing/stdlib-public-api.json \
    --slurpfile matrix testing/stdlib-matrix.json \
    '
    def laws($id):
      {
        "std.meta": ["canonical-structure", "source-builder", "budgeted-generation"],
        "std.reflect": ["public-closure", "privacy-boundary", "artifact-identity"],
        "std.bytes": ["byte-identity", "utf8-boundary", "bounded-slicing"],
        "std.time": ["monotonic-order", "provider-equivalence", "timer-lifecycle"],
        "std.env": ["snapshot-isolation", "name-encoding", "budget-atomicity"],
        "std.core": ["option-result-algebra", "generic-protocols", "terminal-composition"],
        "std.text": ["scalar-indexing", "utf8-boundary", "ascii-transform"],
        "std.collections": ["value-copy-cow", "insertion-order", "key-membership"],
        "std.iter": ["lazy-consumption", "adapter-composition", "bounded-materialization"],
        "std.math": ["ieee-rounding", "domain-boundary", "deterministic-special-values"],
        "std.format": ["bounded-builder", "display-atomicity", "join-order"],
        "std.io": ["short-io-progress", "limit-boundary", "flush-atomicity"],
        "std.async": ["effect-visible-suspension", "structured-handle-ownership", "bounded-stream-close"],
        "std.console": ["stream-separation", "line-termination", "capability-boundary"],
        "std.path": ["byte-snapshot", "lexical-normalization", "join-boundary"],
        "std.fs": ["handle-lifecycle", "directory-order", "atomic-write"],
        "std.process": ["pipe-topology", "stream-separation", "reaping-lifecycle"],
        "std.serialization": ["event-protocol", "path-diagnostics", "atomic-publication"],
        "std.json": ["rfc-value-model", "canonical-order", "fragmented-streaming"],
        "std.messagepack": ["wire-value-model", "minimal-forms", "fragmented-streaming"],
        "std.protobuf": ["schema-wire-model", "unknown-preservation", "fragmented-streaming"],
        "std.testing": ["assertion-observations", "replayable-generation", "isolated-cleanup"]
      }[$id];

    def campaigns($id):
      {
        "std.meta": ["protocols"],
        "std.reflect": ["bounded-owner-corpus"],
        "std.bytes": ["bounded-owner-corpus"],
        "std.time": ["bounded-owner-corpus"],
        "std.env": ["bounded-owner-corpus"],
        "std.core": ["admission"],
        "std.text": ["admission", "bounded-owner-corpus"],
        "std.collections": ["admission"],
        "std.iter": ["admission"],
        "std.math": ["bounded-owner-corpus"],
        "std.format": ["bounded-owner-corpus"],
        "std.io": ["bounded-owner-corpus"],
        "std.async": ["bounded-owner-corpus"],
        "std.console": ["bounded-owner-corpus"],
        "std.path": ["bounded-owner-corpus"],
        "std.fs": ["bounded-owner-corpus"],
        "std.process": ["bounded-owner-corpus"],
        "std.serialization": ["stdlib_codecs"],
        "std.json": ["stdlib_codecs"],
        "std.messagepack": ["stdlib_codecs"],
        "std.protobuf": ["stdlib_codecs"],
        "std.testing": ["bounded-owner-corpus"]
      }[$id];

    ($evidence[0]) as $e
    | ($api[0]) as $a
    | ($matrix[0]) as $m
    | ($e.owners | map(.id) | sort) as $owner_ids
    | ($e.owners | map(. as $owner
        | ($owner.id) as $id
        | (first($e.leaves[] | select((.owners | index($id)) != null))) as $leaf
        | ([ $a.rows[] | select(.owner == $id) | {id, symbol, signature} ] | sort_by(.id)) as $public_api
        | ([ $m.rows[] | select(.owner == $id and .kind == "requirement") | .id ] | sort) as $requirements
        | {
            id: $id,
            leaf: $leaf.id,
            contract: $leaf.contract,
            public_api: $public_api,
            requirements: $requirements,
            model: {
              status: $owner.cells.MODEL.status,
              laws: laws($id),
              refs: (($owner.cells.MODEL.refs + ["crates/tondo-reliability/tests/stdlib_owner_models.rs"]) | unique)
            },
            test: {
              status: $owner.cells.TEST.status,
              commands: $owner.commands,
              refs: $owner.cells.TEST.refs
            },
            fuzz: {
              status: $owner.cells.FUZZ.status,
              reason: $owner.cells.FUZZ.reason,
              campaigns: campaigns($id),
              refs: $owner.cells.FUZZ.refs
            }
          }) | sort_by(.id)) as $owners
    | {
        format: "tondo-stdlib-test-coordination/1",
        edition: "0.1",
        phase: "STD-0.1A",
        status: "closed-coordination",
        sources: {
          owner_evidence: "testing/stdlib-owner-evidence.json",
          public_api: "testing/stdlib-public-api.json",
          normative_matrix: "testing/stdlib-matrix.json",
          model_test: "crates/tondo-reliability/tests/stdlib_owner_models.rs"
        },
        rules: {
          one_owner_per_surface: true,
          every_surface_has_model_law: true,
          every_owner_has_test_commands: true,
          fuzz_gaps_require_reason: true,
          partial_fuzz_is_not_promotion: true
        },
        owners: $owners,
        summary: {
          owners: ($owners | length),
          public_signatures: ([$owners[].public_api[]] | length),
          owner_requirements: ([$owners[].requirements[]] | length),
          model_laws: ([$owners[].model.laws[]] | length),
          fuzz_verified: ([$owners[] | select(.fuzz.status == "verified")] | length),
          fuzz_partial: ([$owners[] | select(.fuzz.status == "partial")] | length)
        },
        next_coordination: "STD-A-PERF-001"
      }
    ' > "$output"

tail -c 1 "$output" | cmp -s <(printf '\n') || {
    echo "stdlib test coordination: generated output must end with LF" >&2
    exit 1
}

echo "stdlib test coordination generated: $output"
