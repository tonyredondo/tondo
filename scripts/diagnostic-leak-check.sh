#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_LEAK_CONTRACT:-$root/testing/diagnostic-leak.json}"

die() {
    echo "diagnostic leak: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing leak registry: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-leak/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "LEAK-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-leak.md"
  and (.implementation | length) == 4
  and (.tests | length) == 4
  and .algorithm == {
    heap: "generational-tracing-gc",
    snapshots: "quiescence-end-with-roots",
    retention: "object-visible-in-two-snapshots-and-sustained-growth",
    resources: "final-acquired-without-release",
    observed_only: true
  }
  and .finding_kinds == [
    "managed-retention", "affine-resource", "native-allocation", "sustained-growth"
  ]
  and .required_context == [
    "allocation-stack", "owner", "retainers", "cleanup-state",
    "size", "first-view", "last-view", "quiescence"
  ]
  and .status_values == ["clean", "finding", "unsupported"]
  and .limits == {
    max_observations: 100000,
    max_findings: 100000,
    max_events: 1000000,
    max_stack_depth: 256,
    max_retainers_per_object: 256,
    min_growth_snapshots: 3,
    fail_closed: true
  }
  and .privacy == {
    payloads: "omitted-by-default",
    physical_paths: "omitted-by-default",
    network_upload: false
  }
  and .lifecycle == {
    fresh_process_per_attempt: true,
    retry_state_isolated: true,
    shard_state_isolated: true,
    suite_state_isolated: true
  }
  and .public_stdlib_api == false
  and .native_parity == "DIAG-NATIVE-001"
  and .next_blocks == ["DIAG-CI-001"]
  and ((.unsupported_reasons | length) == 7)
' "$contract" >/dev/null || die "invalid leak registry"

for path in \
    docs/contracts/diagnostic-leak.md \
    crates/tondo-vm/src/runtime/leak.rs \
    crates/tondo-vm/src/runtime/diagnostics.rs \
    crates/tondo-vm/src/runtime/execute.rs \
    crates/tondo-vm/src/runtime.rs; do
    [[ -f "$root/$path" ]] || die "missing leak evidence: $path"
done

grep -Fq 'pub fn detect_leaks' "$root/crates/tondo-vm/src/runtime/leak.rs" \
    || die "leak analysis entry point is absent"
grep -Fq 'sustained_growth' "$root/crates/tondo-vm/src/runtime/leak.rs" \
    || die "sustained-growth analysis is absent"
grep -Fq 'DiagnosticQuiescencePhase::End' "$root/crates/tondo-vm/src/runtime/leak.rs" \
    || die "quiescence boundary is absent"
grep -Fq 'mod leak;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "leak module is not wired into the runtime"
! grep -Fq 'pub mod leak;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "leak hooks must not be a public module"

echo "diagnostic leak: OK (hosted GC retention, quiescent growth, resources and fail-closed limits)"
