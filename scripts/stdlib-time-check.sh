#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_TIME_CONTRACT:-testing/stdlib-time.json}"

die() {
    echo "std.time owner contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: $contract"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.time"
  and .edition == "0.1"
  and .phase == "STD-0.1A"
  and .status == "draft-contract"
  and .contract == "docs/contracts/stdlib-time.md"
  and .layer == "A0"
  and .kind == "capability-gated"
  and .target == "tondo-vm-hosted"
  and .profile == "time"
  and .api == "tondo-std-time-0.1/1"
  and .source.path == "crates/tondo-compiler/src/process_host.rs"
  and .capabilities.required == ["clock"]
  and ((.capabilities.forbidden | index("clock")) == null)
  and ((.capabilities.forbidden | index("ambient-host")) != null)
  and ((.capabilities.forbidden | index("runtime-value-reflection")) != null)
  and (.invariants | length) == 9
  and ([.limits[].id] | unique) == [
    "active_time_resources", "clock_resolution", "duration_nanoseconds",
    "instant_value", "virtual_advance"
  ]
  and ([.test_matrix[].id] | unique) == [
    "capability-and-conformance", "limits-and-errors", "model-and-identity",
    "real-provider", "timers-and-cancellation", "virtual-provider"
  ]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == [
    "provider-equivalence", "time-arithmetic-boundaries", "timer-lifecycle"
  ]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_coordination == "STD-A-ENV-EVIDENCE-001"
' "$contract" >/dev/null || die "invalid owner contract"

source_hash="sha256:$(sha256sum "$(jq -r '.source.path' "$contract")" | cut -d ' ' -f 1)"
declared_source_hash="$(jq -r '.source.sha256' "$contract")"
[[ "$declared_source_hash" == "$source_hash" ]] || die "owner source hash does not match source"

[[ -f "$(jq -r '.contract' "$contract")" ]] || die "missing normative contract document"
[[ -f "$(jq -r '.source.path' "$contract")" ]] || die "missing time implementation source"

echo "std.time owner contract: OK"
