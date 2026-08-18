#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_LINK_PLAN_CONTRACT:-$root/testing/native-link-plan.json}"

[[ -f "$contract" ]] || { echo "missing native link plan contract: ${contract#"$root"/}" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || {
    echo "native link plan contract must end with one LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || {
    echo "native link plan contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-native-link-plan-contract/1"
  and .owner == "toolchain.native_link_plan"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .descriptor_format == "tondo-native-link-plan-draft"
  and .canonical_encoding == "compact-utf8-json-struct-order"
  and .record_identity == "sha256(canonical-native-link-plan-bytes)"
  and .semantic_identity == "plan_hash=sha256(canonical-link-plan-fingerprint)"
  and .required_fields == [
    "format", "compiler", "edition", "package_id",
    "target_descriptor_hash", "artifact_hash",
    "artifact_target_descriptor_hash", "inputs", "driver", "output",
    "limits", "plan_hash", "reproducible"
  ]
  and .input_fields == ["id", "kind", "sha256"]
  and .driver_fields == ["id", "version", "artifact_id", "artifact_sha256", "arguments"]
  and .output_fields == ["product_id", "object_format", "expected_sha256"]
  and .limit_fields == ["max_inputs", "max_arguments", "max_output_bytes"]
  and .input_kinds == ["object", "privileged-unit", "runtime", "stdlib"]
  and .input_order == [
    "objects-in-compiler-order",
    "privileged-units-in-declared-order",
    "one-runtime",
    "one-stdlib"
  ]
  and .bindings.target_descriptor == "target_descriptor_hash-equals-artifact-target-descriptor-hash"
  and .bindings.artifact == "artifact_hash-identifies-native-artifact"
  and .bindings.driver == "exact-target-driver-identity-and-ordered-arguments"
  and .bindings.inputs == "ordered-hash-pinned-subset-of-native-artifact-link-inputs"
  and .bindings.output == "logical-product-id-object-format-and-expected-hash"
  and .selection.filesystem_lookup == "forbidden"
  and .selection.path_lookup == "forbidden"
  and .selection.environment_lookup == "forbidden"
  and .selection.shell_expansion == "forbidden"
  and .selection.shell_execution == "forbidden"
  and .limits.positive == true
  and .limits.must_cover_inputs == true
  and .limits.must_cover_arguments == true
  and .limits.finite_output_bytes == true
  and .reproducible == true
  and (.negative_cases | sort) == [
    "driver-mismatch", "duplicate-input", "invalid-input-order",
    "missing-hash", "mixed-targets", "non-portable-path", "output-mismatch",
    "pretty-or-unsorted-record", "stale-plan-hash", "unknown-fields",
    "unsupported-object-format", "zero-or-insufficient-limit"
  ]
  and .next_blocks == ["NATIVE-001"]
' "$contract" >/dev/null

source="$root/crates/tondo-compiler/src/toolchain.rs"
for symbol in \
    'pub const NATIVE_LINK_PLAN_FORMAT' \
    'pub struct NativeLinkInput' \
    'pub struct NativeLinkDriver' \
    'pub struct NativeLinkOutput' \
    'pub struct NativeLinkLimits' \
    'pub struct NativeLinkPlan' \
    'pub fn calculated_plan_hash'; do
    grep -Fq "$symbol" "$source" || {
        echo "missing native link plan symbol: $symbol" >&2
        exit 1
    }
done

grep -Fq 'NATIVE-PUBLISH-SPEC-001' "$root/docs/contracts/native-link-plan.md"
grep -Fq 'physical paths' "$root/docs/contracts/native-link-plan.md"

echo "native link plan contract: OK"
