#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_ARTIFACT_CONTRACT:-$root/testing/native-artifact.json}"

[[ -f "$contract" ]] || { echo "missing native artifact contract: ${contract#"$root"/}" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || {
    echo "native artifact contract must end with one LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || {
    echo "native artifact contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-native-artifact-contract/1"
  and .owner == "toolchain.native_artifact"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .descriptor_format == "tondo-native-artifact-draft"
  and .canonical_encoding == "compact-utf8-json-struct-order"
  and .record_identity == "sha256(canonical-native-artifact-bytes)"
  and .semantic_identity == "artifact_hash=sha256(canonical-graph-fingerprint)"
  and .required_fields == [
    "format", "compiler", "edition", "package_id",
    "target_descriptor_hash", "source_artifact_hash", "nodes", "producers",
    "product_id", "artifact_hash", "reproducible"
  ]
  and .node_fields == ["id", "kind", "role", "sha256", "producer"]
  and .producer_fields == ["id", "kind", "inputs", "outputs", "sha256"]
  and .node_kinds == ["object", "runtime", "stdlib", "privileged-unit", "product"]
  and .node_roles == ["input", "intermediate", "output"]
  and .producer_kinds == ["compile", "prepare", "link"]
  and .graph.nodes == "sorted-unique-by-id"
  and .graph.producers == "sorted-unique-by-id"
  and .graph.producer_edges == "sorted-unique-and-acyclic"
  and .graph.reachability == "every-node-and-producer-reaches-product"
  and .graph.required_inputs == "object-plus-one-runtime-plus-one-stdlib"
  and .graph.product == "one-output-product-owned-by-one-link-producer"
  and .graph.reproducible == true
  and .identity.target_descriptor == "sha256-of-native-target-descriptor"
  and .identity.source_artifact == "sha256-of-tondo-artifact-draft"
  and .identity.node_and_producer_hashes == "validated-identities-no-path-lookup"
  and .layout.physical_paths == "forbidden"
  and .layout.object_layout == "not-public"
  and .layout.calling_convention == "not-public"
  and .layout.ffi_abi == "not-public"
  and (.negative_cases | sort) == [
    "input-overwrite", "invalid-hash", "invalid-role-kind-combination",
    "missing-runtime-or-stdlib", "multiple-product-producers",
    "non-link-product-producer", "orphan-node-or-producer", "physical-layout-field",
    "pretty-or-unsorted-record", "producer-cycle", "unknown-fields",
    "unknown-node-or-producer"
  ]
  and .next_blocks == ["NATIVE-LINK-PLAN-001", "NATIVE-PUBLISH-SPEC-001"]
' "$contract" >/dev/null

source="$root/crates/tondo-compiler/src/toolchain.rs"
for symbol in \
    'pub const NATIVE_ARTIFACT_FORMAT' \
    'pub struct NativeArtifactNode' \
    'pub struct NativeArtifactProducer' \
    'pub struct NativeArtifact' \
    'pub fn calculated_artifact_hash'; do
    grep -Fq "$symbol" "$source" || {
        echo "missing native artifact symbol: $symbol" >&2
        exit 1
    }
done

grep -Fq 'NATIVE-LINK-PLAN-001' "$root/docs/contracts/native-artifact.md"
grep -Fq 'physical path' "$root/docs/contracts/native-artifact.md"

echo "native artifact contract: OK"
