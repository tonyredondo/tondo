#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_EVALUATION_CONTRACT:-$root/testing/native-evaluation.json}"

die() {
    echo "native evaluation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-evaluation/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .phase == "NATIVE-001"
  and .status == "decision-ready"
  and .contract == "docs/contracts/native-evaluation.md"
  and .workflow == ".github/workflows/native-evaluation.yml"
  and .adr == "docs/adr/019-native-backend-selection.md"
  and .fast_lane == "testing/native-evaluation-fast.json"
  and .adapter == {
      format: "tondo-mir-backend/1",
      runner: "testing/native-evaluation-runner.json",
      status: "scalar-and-managed-result-cfg-vm-differential",
      vm_equivalence: "scalar-and-managed-result-cfg-only"
  }
  and .decision.selected_backend == null
  and .decision.selection_scope == "first-native-backend"
  and .decision.selection_status == "decision-recorded-separately"
  and .decision.selection_record == "testing/native-selection.json"
  and .decision.n1_claim == false
  and .decision.native_performance_baseline == "candidate-slice-captured"
  and .decision.next_implementation == "N1"
  and (.decision.reasons | length >= 4 and unique_values)
  and ([.decision.selection_requires[]] | length == 5 and unique_values)
  and ([.candidates[].id] == ["cranelift", "llvm", "custom"])
  and ([.candidates[].role] == ["candidate", "candidate", "excluded"])
  and ([.candidates[] | select(.role == "candidate")] | length == 2)
  and all(.candidates[];
      (.id | test("^[a-z]+$"))
      and (.status | type == "string" and length > 0)
      and (.integration | type == "string" and length > 0)
      and (.debugging | type == "string" and length > 0)
      and (.distribution | type == "string" and length > 0)
  )
  and .mir_probe.format == "tondo-native-mir-probe/1"
  and .mir_probe.source == "crates/tondo-compiler/examples/native_mir_probe.rs"
  and .mir_probe.oracle_backend == "bytecode-vm-oracle"
  and .mir_probe.adapter_format == "tondo-mir-backend/1"
  and (.mir_probe.required_summary_fields | length == 21 and unique_values)
  and (.mir_probe.identity_fields | length == 6 and unique_values)
  and (.mir_probe.forbidden_fields | length >= 7 and unique_values)
  and ([.mir_probe.fixtures[].path] | length == 4 and unique_values)
  and all(.mir_probe.fixtures[];
      (.path | test("^[^/][^\\\\]*\\.to$"))
      and (.sha256 | test("^[0-9a-f]{64}$"))
      and (.required_features | length > 0 and unique_values)
      and all(.required_features[];
          . as $feature
          | ["invoke", "return", "host-call", "iterator-next", "validate-places",
             "await", "spawn", "task-scope", "fallback"] | index($feature) != null)
  )
  and .oracle.compile == "same-MIR-inventory-and-diagnostics"
  and .oracle.runtime == "exact-VM-language-observables"
  and (.oracle.required_before_native_performance ==
      ["values", "errors", "ordering", "ownership", "overflow", "cancellation", "exit-status"])
  and .oracle.mismatch == "fail-closed"
  and ([.evaluation_dimensions[].id] == [
      "target-support", "correctness", "compile-latency", "runtime", "memory",
      "code-size", "debugging", "distribution", "maintenance", "licensing",
      "diagnostic-parity", "source-maps", "task-thread-registry", "memory-gc-hooks",
      "unwind", "redaction", "crash-dumps"
  ])
  and all(.evaluation_dimensions[]; .status | type == "string" and length > 0)
  and .toolchain_policy == {
      "versions": "pinned-before-lowering",
      "path_lookup": "forbidden",
      "environment_lookup": "forbidden",
      "shell_expansion": "forbidden",
      "unhashed_inputs": "forbidden",
      "physical_paths_in_identity": "forbidden"
  }
  and (.negative_cases | length == 9 and unique_values)
  and .next_blocks == ["N1"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-evaluation.md \
    docs/adr/019-native-backend-selection.md \
    docs/contracts/native-selection.md \
    testing/native-selection.json \
    .github/workflows/native-evaluation.yml \
    crates/tondo-compiler/examples/native_mir_probe.rs \
    testing/native-evaluation-runner.json \
    testing/native-std-core.json \
    testing/native-aot-lowering.json \
    testing/native-aot-binary.json \
    docs/contracts/native-aot-lowering.md \
    docs/contracts/native-aot-binary.md \
    scripts/native-aot-lowering-check.sh \
    scripts/native-aot-lowering-test.sh \
    scripts/native-aot-binary-check.sh \
    scripts/native-aot-binary-test.sh \
    scripts/native-evaluation-runner.sh \
    scripts/native-evaluation-runner-check.sh \
    scripts/native-evaluation-runner-test.sh; do
    [[ -f "$root/$path" ]] || die "missing evaluation evidence: $path"
done
[[ -f "$root/testing/native-evaluation-fast.json" ]] || die "missing fast-lane contract"
scripts/native-evaluation-fast-check.sh >/dev/null || die "fast-lane contract is invalid"

grep -Fq 'pub struct MirSummary' "$root/crates/tondo-compiler/src/mir.rs" \
    || die "MIR summary type is missing"
grep -Fq 'pub fn summary(&self) -> MirSummary' "$root/crates/tondo-compiler/src/mir.rs" \
    || die "MIR summary accessor is missing"
grep -Fq 'pub fn mir_summary(&self)' "$root/crates/tondo-compiler/src/driver.rs" \
    || die "compilation output does not expose the MIR summary"
grep -Fq 'tondo-native-mir-probe/1' "$root/crates/tondo-compiler/examples/native_mir_probe.rs" \
    || die "probe format is missing"
grep -Fq 'bytecode-vm-oracle' "$root/crates/tondo-compiler/examples/native_mir_probe.rs" \
    || die "VM oracle is missing"
grep -Fq 'with_bytecode_observation' \
    "$root/crates/tondo-compiler/examples/native_mir_probe.rs" \
    || die "probe does not retain verified bytecode for differential execution"
grep -Fq 'execute_with_arguments' \
    "$root/crates/tondo-compiler/examples/native_mir_probe.rs" \
    || die "probe does not invoke the VM scalar oracle"
grep -Fq 'workflow_dispatch:' "$root/.github/workflows/native-evaluation.yml" \
    || die "native evaluation workflow must be opt-in"
! grep -Eq '^  push:' "$root/.github/workflows/native-evaluation.yml" \
    || die "native evaluation workflow must not run on every push"

while IFS=$'\t' read -r fixture_path expected_sha; do
    [[ -n "$fixture_path" && -n "$expected_sha" ]] || die "empty fixture record"
    fixture="$root/$fixture_path"
    [[ -f "$fixture" ]] || die "missing fixture: $fixture_path"
    actual_sha="$(sha256sum "$fixture" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "fixture hash mismatch: $fixture_path"
done < <(jq -r '.mir_probe.fixtures[] | [.path, .sha256] | @tsv' "$contract")

while IFS=$'\t' read -r fixture_path required_features; do
    for feature in $required_features; do
        [[ "$feature" =~ ^[a-z0-9-]+$ ]] || die "invalid required feature: $fixture_path/$feature"
    done
done < <(jq -r '.mir_probe.fixtures[] | [.path, (.required_features | join(" "))] | @tsv' "$contract")

echo "native evaluation: OK (candidate evidence lane does not auto-select; DEC-013 records Cranelift and Gate N1 promotion is composed separately)"
