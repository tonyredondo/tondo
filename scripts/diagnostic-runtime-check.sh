#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_RUNTIME_CONTRACT:-$root/testing/diagnostic-runtime.json}"

die() {
    echo "diagnostic runtime: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing runtime registry: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-runtime/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DIAG-RUNTIME-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-runtime.md"
  and (.implementation | length) == 4
  and (.tests | length) == 4
  and .schema == "tondo-diagnostic-runtime/1"
  and .event_kinds == ["thread", "task", "memory", "synchronization", "heap", "roots", "resource", "scheduler", "quiescence"]
  and .required_context == ["task-thread-ids", "memory-accesses", "synchronization", "roots-retainers", "resource-ledger", "source-maps", "scheduler-events", "quiescence-barriers"]
  and .limits == {
    max_events: 1000000,
    max_stack_depth: 256,
    max_retainers_per_object: 256,
    max_scheduler_tail_events: 4096,
    events_fail_closed: true,
    truncation_reported: true
  }
  and .privacy == {payloads: "omitted-by-default", logical_spans_only: true, network_upload: false}
  and .hooks_private == true
  and .normal_path == "no-collector-and-no-events"
  and .semantic_equivalence == true
  and .native_parity == "DIAG-NATIVE-001"
  and .next_blocks == ["DIAG-TEST-001", "DIAG-CI-001"]
' "$contract" >/dev/null || die "invalid runtime registry"

for path in \
    docs/contracts/diagnostic-runtime.md \
    crates/tondo-vm/src/runtime/diagnostics.rs \
    crates/tondo-vm/src/runtime/execute.rs \
    crates/tondo-vm/src/runtime/heap.rs \
    crates/tondo-vm/src/runtime.rs; do
    [[ -f "$root/$path" ]] || die "missing runtime evidence: $path"
done

grep -Fq 'execute_with_diagnostics' "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "diagnostic execution entry point is absent"
grep -Fq 'pub diagnostics: Option<DiagnosticTrace>' "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "execution result does not carry an optional trace"
grep -Fq 'max_events: 1_000_000' "$root/crates/tondo-vm/src/runtime/diagnostics.rs" \
    || die "D0 event budget is absent"
grep -Fq 'mod diagnostics;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "diagnostic module is not private"
! grep -Fq 'pub mod diagnostics;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "diagnostic hooks must not be a public module"

echo "diagnostic runtime: OK (bounded VM events, roots/resources/source maps, private opt-in hooks)"
