#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENV_CONTRACT:-testing/stdlib-env.json}"

die() {
    echo "std.env owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.env"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-contract"
  and .contract == "docs/contracts/stdlib-env.md"
  and .layer == "A0"
  and .kind == "capability-gated"
  and .target == "tondo-vm-hosted"
  and .profile == "environment"
  and .api == "tondo-std-env-0.1/1"
  and .source.path == "crates/tondo-compiler/src/process_host.rs"
  and .capabilities.required == ["environment"]
  and ((.capabilities.forbidden | index("environment")) == null)
  and ((.capabilities.forbidden | index("ambient-host")) != null)
  and ((.capabilities.forbidden | index("runtime-value-reflection")) != null)
  and (.invariants | length) == 9
  and ([.limits[].id] | unique) == ["name_bytes", "snapshot_bytes", "value_copy_bytes"]
  and ([.test_matrix[].id] | unique) == [
    "arguments-and-order", "capability-and-availability", "limits-and-atomicity",
    "missing-and-copying", "names-and-values", "snapshot-identity-and-sealing"
  ]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["budget-atomicity", "name-boundaries", "snapshot-inputs"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_coordination == "STD-A-CORE-EVIDENCE-001"
' "$contract" >/dev/null || die "invalid owner contract"

source_hash="sha256:$(sha256sum "$(jq -r '.source.path' "$contract")" | cut -d ' ' -f 1)"
declared_source_hash="$(jq -r '.source.sha256' "$contract")"
[[ "$declared_source_hash" == "$source_hash" ]] || die "owner source hash does not match source"

[[ -f "$(jq -r '.contract' "$contract")" ]] || die "missing normative contract document"
[[ -f "$(jq -r '.source.path' "$contract")" ]] || die "missing environment implementation source"

echo "std.env owner contract: OK"
