#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_BYTES_CONTRACT:-testing/stdlib-bytes.json}"

die() {
    echo "std.bytes owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.bytes"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-contract"
  and .contract == "docs/contracts/stdlib-bytes.md"
  and .layer == "A0"
  and .kind == "intrinsic"
  and .target == "tondo-vm-hosted"
  and .profile == "bytes"
  and .api == "tondo-std-bytes-0.1/1"
  and .source.path == "crates/tondo-compiler/src/process_host.rs"
  and (.capabilities.required | length) == 0
  and (.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length)
  and ((.capabilities.forbidden | index("ambient-host")) != null)
  and ((.capabilities.forbidden | index("runtime-value-reflection")) != null)
  and (.invariants | length) == 8
  and ([.limits[].id] | unique) == [
    "array_copy_bytes", "builder_append_bytes", "hash_work_units", "index",
    "materialized_bytes", "slice_extent"
  ]
  and ([.test_matrix[].id] | unique) == [
    "builder-atomicity", "catalog-shape", "limits-and-ranges",
    "ownership-and-snapshots", "properties-and-hot-paths", "utf8-conversion"
  ]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["builder-failure-atomicity", "bytes-boundaries", "utf8-corpus"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_coordination == "STD-A-TIME-EVIDENCE-001"
' "$contract" >/dev/null || die "invalid owner contract"

source_hash="sha256:$(sha256sum "$(jq -r '.source.path' "$contract")" | cut -d ' ' -f 1)"
declared_source_hash="$(jq -r '.source.sha256' "$contract")"
[[ "$declared_source_hash" == "$source_hash" ]] || die "owner source hash does not match source"

[[ -f "$(jq -r '.contract' "$contract")" ]] || die "missing normative contract document"
[[ -f "$(jq -r '.source.path' "$contract")" ]] || die "missing bytes implementation source"

echo "std.bytes owner contract: OK"
