#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
contract="${TONDO_NATIVE_AOT_SCOPE_CONTRACT:-$root/testing/native-aot-scope.json}"

die() {
    echo "native AOT scope: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-aot-scope/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .phase == "M20"
  and .status == "closed-contract"
  and .product == {
    primary: "native-aot",
    reference: "tondo-vm-hosted",
    jit: "out-of-scope"
  }
  and .memory == {
    language: "collector-neutral",
    native_aot: "hybrid-arc-cycle-collector",
    vm: "precise-tracing-stop-the-world-mark-and-sweep"
  }
  and .candidates == {included: ["cranelift", "llvm"], excluded: ["custom"]}
  and (.identity_fields | length == 10 and unique_values)
  and .identity_fields == [
    "edition", "target", "backend", "profile", "toolchain", "runtime",
    "stdlib", "fixture", "flags", "source_revision"
  ]
  and (.dimensions | length == 8 and unique_values)
  and .dimensions == [
    "compile-latency", "linked-binary-size", "startup", "runtime", "memory",
    "diagnostic-parity", "maintenance", "distribution"
  ]
  and .binary_size_metrics == ["stripped-bytes", "debug-bytes", "section-bytes"]
  and .protocol == {
    warmups: 3,
    samples_per_process: 9,
    processes: 3,
    minimum_samples: 27,
    fresh_processes: true,
    same_inputs: true
  }
  and .required_blocks == [
    "NATIVE-AOT-LOWER-001",
    "NATIVE-AOT-BINARY-001",
    "NATIVE-AOT-MEM-001",
    "NATIVE-AOT-QUALITY-001",
    "NATIVE-AOT-PERF-001"
  ]
  and .selection == {
    selected_backend: null,
    n1_claim: false,
    status: "pending-aot-evidence"
  }
  and (.negative_cases | length == 10 and unique_values)
  and .next_blocks == ["NATIVE-AOT-LOWER-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-scope.md \
    docs/adr/019-native-backend-selection.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_TOOLCHAIN_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing scope evidence: $path"
done

grep -Fq 'native AOT' "$root/docs/contracts/native-aot-scope.md" \
    || die "scope contract does not state the AOT product boundary"
grep -Fq 'JIT' "$root/docs/contracts/native-aot-scope.md" \
    || die "scope contract does not state the JIT boundary"
grep -Fq 'hybrid-arc-cycle-collector' "$root/docs/contracts/native-aot-scope.md" \
    || die "scope contract does not state native memory policy"
grep -Fq 'NATIVE-AOT-SCOPE-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the AOT scope block"

jq -e '
  .task_dependencies["NATIVE-AOT-SCOPE-001"] == ["NATIVE-001", "PERF-001", "NATIVE-MEM-ADR-001"]
  and ((.task_dependencies["NATIVE-AOT-LOWER-001"] | index("NATIVE-AOT-SCOPE-001")) != null)
' "$root/testing/tracker-graph.json" >/dev/null \
    || die "tracker graph does not gate lowering on AOT scope"

echo "native AOT scope: OK"
