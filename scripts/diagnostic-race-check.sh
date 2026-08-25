#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_RACE_CONTRACT:-$root/testing/diagnostic-race.json}"

die() {
    echo "diagnostic race: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing race registry: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-race/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "RACE-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-race.md"
  and (.implementation | length) == 4
  and (.tests | length) == 4
  and .algorithm == {
    clock: "vector-clock",
    ordering: "happens-before",
    conflict: "same-storage-one-write-different-task-without-hb",
    observed_only: true
  }
  and .accesses == ["Read", "Write", "Move"]
  and .location_identity == {
    shared: "storage_id+path_hash",
    local: "task_id+frame+slot+path_hash"
  }
  and .required_context == [
    "memory-accesses", "task-lifecycle", "synchronization", "source-map",
    "access-stack", "creation-stack"
  ]
  and .status_values == ["clean", "finding", "unsupported"]
  and .limits == {
    max_observations: 100000,
    max_findings: 100000,
    max_events: 1000000,
    max_stack_depth: 256,
    fail_closed: true
  }
  and .privacy == {payloads: "omitted-by-default", network_upload: false}
  and .public_stdlib_api == false
  and .native_parity == "DIAG-NATIVE-001"
  and .next_blocks == ["DIAG-CI-001"]
  and ((.unsupported_reasons | length) == 7)
' "$contract" >/dev/null || die "invalid race registry"

for path in \
    docs/contracts/diagnostic-race.md \
    crates/tondo-vm/src/runtime/race.rs \
    crates/tondo-vm/src/runtime/diagnostics.rs \
    crates/tondo-vm/src/runtime/execute.rs \
    crates/tondo-vm/src/runtime.rs; do
    [[ -f "$root/$path" ]] || die "missing race evidence: $path"
done

grep -Fq 'struct Clock' "$root/crates/tondo-vm/src/runtime/race.rs" \
    && grep -Fq 'happens_before' "$root/crates/tondo-vm/src/runtime/race.rs" \
    || die "vector-clock implementation is absent"
grep -Fq 'pub fn detect_races' "$root/crates/tondo-vm/src/runtime/race.rs" \
    || die "race analysis entry point is absent"
grep -Fq 'storage_id' "$root/crates/tondo-vm/src/runtime/diagnostics.rs" \
    || die "shared storage identity is absent"
grep -Fq 'mod race;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "race module is not wired into the runtime"
! grep -Fq 'pub mod race;' "$root/crates/tondo-vm/src/runtime.rs" \
    || die "race hooks must not be a public module"

echo "diagnostic race: OK (hosted VM vector clocks, bounded identities, stacks and fail-closed limits)"
