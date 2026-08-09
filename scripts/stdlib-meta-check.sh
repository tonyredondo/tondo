#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_META_CONTRACT:-testing/stdlib-meta.json}"
descriptor="${TONDO_STDLIB_META_DESCRIPTOR:-stdlib/meta/descriptor.json}"
source="${TONDO_STDLIB_META_SOURCE:-stdlib/meta/src/meta.to}"

die() {
    echo "std.meta owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
[[ -f "$descriptor" ]] || die "missing package descriptor: $descriptor"
[[ -f "$source" ]] || die "missing package source: $source"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.meta"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-contract"
  and .contract == "docs/contracts/std-meta.md"
  and .layer == "A0"
  and .kind == "build-only"
  and .target == "tondo-meta"
  and .profile == "meta"
  and .api == "tondo-std-meta-0.1/1"
  and .package.format == "tondo-std-meta-package-draft"
  and .package.package_id == "toolchain:std-meta:draft"
  and .package.source == "stdlib/meta/src/meta.to"
  and .package.descriptor == "stdlib/meta/descriptor.json"
  and .capabilities.required == []
  and (.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length)
  and (.invariants | length) == 8
  and ([.limits[].id] | unique) == [
    "fuzz_input_bytes", "meta_memory_bytes", "meta_output_bytes", "meta_steps",
    "renderer_indentation_level"
  ]
  and ([.test_matrix[].id] | unique) == [
    "canonical-rendering", "compile-time-budget", "meta-target-boundary",
    "request-response", "snapshot-model", "source-builder"
  ]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["meta-protocols", "source-builder-boundaries", "target-boundary"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_coordination == "STD-TEST-001"
' "$contract" >/dev/null || die "invalid owner contract"

source_hash="sha256:$(sha256sum "$source" | cut -d ' ' -f 1)"
descriptor_source_hash="$(jq -r '.source.sha256' "$descriptor")"
[[ "$source_hash" == "$descriptor_source_hash" ]] || die "descriptor source hash does not match source"

declared_source_hash="$(jq -r '.package.source_hash' "$contract")"
[[ "$declared_source_hash" == "$source_hash" ]] || die "owner source hash does not match source"

content_json="$(jq -c '{api,format,package_id,profile,source,target}' "$descriptor")"
content_hash="sha256:$(printf '%s' "$content_json" | sha256sum | cut -d ' ' -f 1)"
descriptor_content_hash="$(jq -r '.content_hash' "$descriptor")"
[[ "$content_hash" == "$descriptor_content_hash" ]] || die "descriptor content hash is not canonical"

declared_content_hash="$(jq -r '.package.content_hash' "$contract")"
[[ "$declared_content_hash" == "$content_hash" ]] || die "owner content hash does not match descriptor"

echo "std.meta owner contract: OK"
