#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_REFLECT_CONTRACT:-testing/stdlib-reflect.json}"

die() {
    echo "std.reflect owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.reflect"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-contract"
  and .contract == "docs/contracts/std-reflect.md"
  and .layer == "A0"
  and .kind == "metadata-only"
  and .target == "tondo-compiler"
  and .profile == "reflect"
  and .api == "tondo-std-reflect-0.1/1"
  and .source.path == "crates/tondo-compiler/src/reflect.rs"
  and (.capabilities.required | length) == 0
  and (.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length)
  and ((.capabilities.forbidden | index("runtime-value-reflection")) != null)
  and (.invariants | length) == 8
  and ([.limits[].id] | unique) == [
    "descriptor_text_bytes", "generic_argument_count", "link_work_units",
    "member_count", "retained_type_count"
  ]
  and ([.test_matrix[].id] | unique) == [
    "artifact-local-identity", "catalog-shape", "malformed-inputs-and-cost",
    "no-runtime-reflection", "privacy", "root-closure"
  ]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["reflect-boundaries", "reflect-catalogs", "root-closures"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_coordination == "STD-TEST-001"
' "$contract" >/dev/null || die "invalid owner contract"

source_hash="sha256:$(sha256sum "$(jq -r '.source.path' "$contract")" | cut -d ' ' -f 1)"
declared_source_hash="$(jq -r '.source.sha256' "$contract")"
[[ "$declared_source_hash" == "$source_hash" ]] || die "owner source hash does not match source"

[[ -f "$(jq -r '.contract' "$contract")" ]] || die "missing normative contract document"
[[ -f "$(jq -r '.source.path' "$contract")" ]] || die "missing reflection implementation source"

echo "std.reflect owner contract: OK"
