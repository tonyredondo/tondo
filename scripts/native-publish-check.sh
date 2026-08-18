#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_PUBLISH_CONTRACT:-$root/testing/native-publish.json}"

[[ -f "$contract" ]] || { echo "missing native publish contract: ${contract#"$root"/}" >&2; exit 1; }
tail -c 1 "$contract" | cmp -s <(printf '\n') || {
    echo "native publish contract must end with one LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || {
    echo "native publish contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-native-publish-contract/1"
  and .owner == "toolchain.native_publish"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .plan_format == "tondo-native-publish-plan-draft"
  and .receipt_format == "tondo-native-published-product-draft"
  and .canonical_encoding == "compact-utf8-json-struct-order"
  and .plan_identity == "plan_hash=sha256(canonical-publish-plan-fingerprint)"
  and .receipt_identity == "receipt_hash=sha256(canonical-published-product-fingerprint)"
  and .record_identity == "sha256(canonical-native-publish-record-bytes)"
  and .required_fields == [
    "format", "compiler", "edition", "package_id",
    "target_descriptor_hash", "artifact_hash", "link_plan_hash",
    "output", "policy", "consumer", "limits", "plan_hash", "reproducible"
  ]
  and .output_fields == ["product_id", "object_format", "expected_sha256"]
  and .policy_fields == ["staging", "commit", "durability", "collision", "interruption", "cleanup"]
  and .consumer_fields == ["command", "receipt_format", "verification", "mismatch"]
  and .limit_fields == ["max_product_bytes", "max_receipt_bytes"]
  and .receipt_fields == [
    "format", "compiler", "edition", "package_id",
    "target_descriptor_hash", "artifact_hash", "link_plan_hash",
    "output", "receipt_hash", "reproducible"
  ]
  and .receipt_output_fields == ["product_id", "object_format", "product_sha256", "product_bytes"]
  and .policy == {
    "staging": "output-sibling",
    "commit": "sync-file-then-atomic-rename",
    "durability": "directory-sync-after-rename-when-supported",
    "collision": "replace-regular-file-or-noop-same-receipt",
    "interruption": "preserve-old-before-commit",
    "cleanup": "remove-staging-on-failure"
  }
  and .consumer == {
    "command": "tondo-run",
    "receipt_format": "tondo-native-published-product-draft",
    "verification": "receipt-and-product-hash-before-exec",
    "mismatch": "reject-before-exec"
  }
  and .limits == {
    "positive": true,
    "finite": true,
    "receipt_checked_before_decode": true,
    "product_checked_before_commit": true
  }
  and .phases == [
    "validate-records", "resolve-regular-output", "stage-product-and-receipt",
    "sync-staged-files", "commit-pair-atomically",
    "sync-parent-directory-when-supported", "cleanup-staging"
  ]
  and .failure_guarantees == {
    "before-commit": "previous-complete-pair-remains-visible",
    "staging-failure": "remove-staging-and-do-not-mutate-product",
    "consumer-mismatch": "reject-before-exec",
    "partial-pair": "never-expose-to-consumer"
  }
  and .collision == {
    "absent": "create",
    "regular_existing": "replace-after-complete-validation",
    "same_receipt": "no-op",
    "directory_or_symlink": "reject-before-staging"
  }
  and .physical_path == "orchestrator-only-out-of-hash"
  and .environment_lookup == "forbidden"
  and .shell_execution == "forbidden"
  and .timestamps == "forbidden"
  and .reproducible == true
  and (.negative_cases | sort) == [
    "consumer-mismatch", "directory-or-symlink-output", "interrupted-stage",
    "invalid-hash", "invalid-policy", "invalid-receipt", "link-plan-mismatch",
    "mixed-targets", "non-positive-limit", "pretty-or-unsorted-record",
    "product-hash-mismatch", "product-size-mismatch", "receipt-hash-mismatch",
    "receipt-over-limit", "stale-plan-hash", "unknown-fields"
  ]
  and .next_blocks == ["PERF-001", "NATIVE-001"]
' "$contract" >/dev/null

source="$root/crates/tondo-compiler/src/toolchain.rs"
for symbol in \
    'pub const NATIVE_PUBLISH_PLAN_FORMAT' \
    'pub const NATIVE_PUBLISHED_PRODUCT_FORMAT' \
    'pub struct NativePublishPolicy' \
    'pub struct NativePublishConsumer' \
    'pub struct NativePublishLimits' \
    'pub struct NativePublishOutput' \
    'pub struct NativePublishPlan' \
    'pub enum NativePublishCollision' \
    'pub struct NativePublishedProduct' \
    'pub struct NativePublishedOutput' \
    'pub fn from_bytes' \
    'pub fn validate_receipt_bytes' \
    'pub fn validate_bytes'; do
    grep -Fq "$symbol" "$source" || {
        echo "missing native publish symbol: $symbol" >&2
        exit 1
    }
done

for marker in \
    'NATIVE-PUBLISH-SPEC-001' \
    'tondo-native-publish-plan-draft' \
    'tondo-native-published-product-draft' \
    'fsync' \
    'atomic' \
    'tondo run'; do
    grep -Fq "$marker" "$root/docs/contracts/native-publish.md" || {
        echo "missing native publish documentation marker: $marker" >&2
        exit 1
    }
done

echo "native publish contract: OK"
