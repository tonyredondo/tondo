#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_NATIVE_N1_CONTRACT:-$root/testing/native-n1.json}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
report="${TONDO_NATIVE_N1_REPORT:-$target_dir/reliability/evidence/native-n1.json}"
arm_evidence="${TONDO_NATIVE_N1_ARM64_EVIDENCE:-$root/target/platform-test/linux-aarch64/native-target.json}"

die() {
    echo "native N1: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing N1 contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "N1 contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "N1 contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-n1-contract/1"
  and .task == "N1"
  and .owner == "toolchain.native_n1"
  and .edition == "0.1"
  and .phase == "M11"
  and .status == "closed-contract"
  and .contract == "docs/contracts/native-n1.md"
  and .runner == "scripts/native-n1.sh"
  and .checker == "scripts/native-n1-check.sh"
  and .tests == "scripts/native-n1-test.sh"
  and .report == "target/reliability/evidence/native-n1.json"
  and .scope == {
      product:"native-aot",
      reference:"tondo-vm-hosted",
      jit:"out-of-scope",
      published_targets:["x86_64-unknown-linux-gnu"],
      candidate_target_smokes:["aarch64-unknown-linux-gnu"]
  }
  and .backend == {
      selected:"cranelift",
      selection_record:"testing/native-selection.json",
      fallback:"forbidden",
      public_abi:false
  }
  and .oracle == {
      vm:"bytecode-vm-oracle",
      mir:"normalized-MIR-reference-interpreter",
      mismatch:"fail-closed"
  }
  and (.required_inputs | length == 21 and unique_values)
  and ([.required_reports[].id] == [
      "NATIVE-EVALUATION-001", "NATIVE-EVALUATION-RUNNER-001",
      "NATIVE-AOT-QUALITY-001", "NATIVE-AOT-PERF-001", "NATIVE-CONF-001",
      "NATIVE-DIFF-001", "NATIVE-REL-001", "NATIVE-TARGET-001",
      "NATIVE-TARGET-002"
  ])
  and all(.required_reports[];
      ((.path | type) == "string" and (.path | startswith("/") | not))
      and ((.format | type) == "string" and (.format | length) > 0)
      and .status == "passed"
      and (.revision_field | IN("git_revision", "source_revision"))
  )
  and .criteria == [
      "aot-campaign", "backend-selection", "shared-abi-mir", "build-run",
      "memory-ownership", "diagnostic-parity", "quality-regression",
      "published-target", "reproducible-package"
  ]
  and .policies == {
      source_revision:"every report must match HEAD",
      workspace:"clean-git-head",
      physical_paths:"forbidden",
      unsupported_admitted:0,
      divergences:0,
      public_release:false,
      arm64_promotion:"candidate-smoke-only-until-complete-aot-corpus"
  }
  and (.negative_cases | length == 15 and unique_values)
  and .next_blocks == ["STD-ASYNC-GROUP-IMPL-001"]
' "$contract" >/dev/null || die "invalid machine-readable N1 contract"

while IFS= read -r input; do
    [[ -n "$input" ]] || continue
    [[ "$input" != /* && "$input" != *..* ]] || die "invalid N1 input path: $input"
    [[ -f "$root/$input" ]] || die "missing N1 input: $input"
done < <(jq -r '.required_inputs[]' "$contract")

if [[ "${1:-}" == "--contract-only" ]]; then
    [[ "$#" -eq 1 ]] || die "--contract-only does not accept additional arguments"
    echo "native N1: contract OK"
    exit 0
fi
[[ "$#" -eq 0 ]] || die "unknown argument: $1"

if [[ "${TONDO_NATIVE_N1_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain=v1 --untracked-files=all)" ]] \
        || die "workspace must be clean before N1 validation"
fi

[[ -f "$report" ]] || die "missing generated N1 report: ${report#"$root"/}"
[[ -f "$arm_evidence" ]] || die "missing ARM64 candidate evidence: ${arm_evidence#"$root"/}"

revision="$(git rev-parse HEAD)"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || die "invalid Git revision"

jq -e --arg revision "$revision" '
  .format == "tondo-native-n1/1"
  and .gate == "N1"
  and .status == "promoted"
  and .edition == "0.1"
  and .phase == "M11"
  and .source_revision == $revision
  and .workspace == "clean"
  and .backend == {
      selected:"cranelift",
      target:"x86_64-unknown-linux-gnu",
      selection_record:"testing/native-selection.json",
      fallback:"forbidden",
      public_abi:false
  }
  and .oracle == {
      vm:"bytecode-vm-oracle",
      mir:"normalized-MIR-reference-interpreter",
      mismatch:"fail-closed"
  }
  and .criteria == [
      {id:"aot-campaign",status:"passed"},
      {id:"backend-selection",status:"passed"},
      {id:"shared-abi-mir",status:"passed"},
      {id:"build-run",status:"passed"},
      {id:"memory-ownership",status:"passed"},
      {id:"diagnostic-parity",status:"passed"},
      {id:"quality-regression",status:"passed"},
      {id:"published-target",status:"passed"},
      {id:"reproducible-package",status:"passed"}
  ]
  and .targets == [
      {
        id:"tondo-native-linux-x86-64-release",
        triple:"x86_64-unknown-linux-gnu",
        object_format:"elf",
        promotion:"promoted",
        physical_smoke:true
      },
      {
        id:"tondo-native-linux-aarch64-release",
        triple:"aarch64-unknown-linux-gnu",
        object_format:"elf",
        promotion:"candidate-smoke-only",
        physical_smoke:true
      }
  ]
  and (.reports | length == 9)
  and ([.reports[].id] == [
      "NATIVE-EVALUATION-001", "NATIVE-EVALUATION-RUNNER-001",
      "NATIVE-AOT-QUALITY-001", "NATIVE-AOT-PERF-001", "NATIVE-CONF-001",
      "NATIVE-DIFF-001", "NATIVE-REL-001", "NATIVE-TARGET-001",
      "NATIVE-TARGET-002"
  ])
  and all(.reports[];
      .status == "passed"
      and ((.path | type) == "string" and (.path | startswith("/") | not))
      and (.sha256 | test("^sha256:[0-9a-f]{64}$"))
      and (.source_revision == $revision)
  )
  and (.inputs | length == 21)
  and ([.inputs[].path] | unique | length) == 21
  and all(.inputs[];
      ((.path | type) == "string" and (.path | startswith("/") | not))
      and (.sha256 | test("^sha256:[0-9a-f]{64}$"))
  )
  and .summary == {
      native_aot_lowering_cases:27,
      native_scalar_cases:118,
      native_managed_cases:3,
      native_runtime_cases:21,
      native_select_cases:8,
      native_thread_cases:5,
      native_std_core_cases:14,
      native_diagnostic_cases:8,
      native_conf_cases:9,
      aot_performance_samples_per_candidate:27,
      quality_mutation_caught:6,
      quality_mutation_missed:0,
      quality_mutation_timeout:0,
      unsupported_admitted:0,
      divergences:0
  }
  and .claims == {
      g5:false,
      s1:false,
      tlf:false,
      public_release:false,
      public_abi:false
  }
  and .physical_paths == []
  and .candidate_target_policy == {
      "aarch64-unknown-linux-gnu":"physical-smoke-only",
      "windows-x86_64":"portable-probe-only",
      "macos-x86_64":"portable-probe-only",
      "macos-aarch64":"portable-probe-only"
  }
  and .next_blocks == ["STD-ASYNC-GROUP-IMPL-001"]
' "$report" >/dev/null || die "generated N1 report is incomplete, stale or overclaims"

while IFS=$'\t' read -r path expected_hash; do
    [[ -n "$path" && -n "$expected_hash" ]] || die "empty N1 input hash record"
    file="$root/$path"
    [[ -f "$file" ]] || die "N1 input disappeared: $path"
    actual_hash="sha256:$(sha256sum "$file" | cut -d ' ' -f1)"
    [[ "$actual_hash" == "$expected_hash" ]] || die "N1 input hash mismatch: $path"
done < <(jq -r '.inputs[] | [.path, .sha256] | @tsv' "$report")

while IFS=$'\t' read -r id path expected_hash expected_revision; do
    [[ -n "$id" && -n "$path" && -n "$expected_hash" && -n "$expected_revision" ]] \
        || die "empty N1 report hash record"
    case "$id" in
        NATIVE-TARGET-002) file="$arm_evidence" ;;
        *) file="$root/$path" ;;
    esac
    [[ -f "$file" ]] || die "N1 report disappeared: $path"
    actual_hash="sha256:$(sha256sum "$file" | cut -d ' ' -f1)"
    [[ "$actual_hash" == "$expected_hash" ]] || die "N1 report hash mismatch: $id"
    [[ "$expected_revision" == "$revision" ]] || die "N1 report revision mismatch: $id"
done < <(jq -r '.reports[] | [.id, .path, .sha256, .source_revision] | @tsv' "$report")

eval_report="$root/target/reliability/evidence/native-evaluation.json"
runner_report="$root/target/reliability/evidence/native-evaluation-runner.json"
quality_report="$root/target/reliability/evidence/native-aot-quality.json"
performance_report="$root/target/reliability/evidence/native-aot-performance.json"
conf_report="$root/target/reliability/evidence/native-conf.json"
diff_report="$root/target/reliability/evidence/native-diff.json"
rel_report="$root/target/reliability/evidence/native-rel.json"
x86_report="$root/target/reliability/evidence/native-target.json"

jq -e --arg revision "$revision" '
  .format == "tondo-native-evaluation-report/1"
  and .status == "passed"
  and .phase == "NATIVE-001"
  and .git_revision == $revision
  and .target == "x86_64-unknown-linux-gnu"
  and .n1_claim == false
  and .mir_probe.format == "tondo-native-mir-probe/1"
  and ([.mir_probe.fixtures[] | select(.status == "passed")] | length == 4)
' "$eval_report" >/dev/null || die "real-MIR evaluation evidence failed N1"

jq -e --arg revision "$revision" '
  .format == "tondo-native-evaluation-candidates/1"
  and .status == "passed"
  and .phase == "NATIVE-001"
  and .source_revision == $revision
  and .target == "x86_64-unknown-linux-gnu"
  and ([.candidates[] | select(.id == "cranelift" or .id == "llvm") | select(.status == "measured")] | length == 2)
  and ([.native_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 118)
  and ([.native_managed_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 3)
  and ([.native_runtime_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 21)
  and ([.native_select_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 8)
  and ([.native_thread_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 5)
  and ([.native_std_core_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 14)
  and ([.native_lowering_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_diagnostics.cases[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 8)
  and .native_diagnostics.status == "passed"
  and .native_aot_lowering.status == "passed"
  and .native_aot_binary.status == "passed"
  and .native_aot_memory.status == "passed"
' "$runner_report" >/dev/null || die "native runner evidence failed N1"

jq -e --arg revision "$revision" '
  .format == "tondo-native-aot-quality/1"
  and .task == "NATIVE-AOT-QUALITY-001"
  and .status == "passed"
  and .source_revision == $revision
  and .native.candidate_status == "both-passed"
  and .native.unsupported_admitted == 0
  and .native.divergences == 0
  and .native.case_counts == {
      native_aot_lowering:27,
      native_scalar:118,
      native_managed:3,
      native_runtime:21,
      native_std_core:14,
      native_diagnostics:8
  }
  and .mutation.status == "passed"
  and .mutation.total == 6
  and .mutation.caught == 6
  and .mutation.missed == 0
  and .mutation.timeout == 0
  and .mutation.unviable == 0
  and .sanitizers.address == "passed"
  and .sanitizers.undefined == "passed"
  and .workspace_quality.baseline_unchanged == true
  and .physical_paths == []
' "$quality_report" >/dev/null || die "native AOT quality evidence failed N1"

jq -e --arg revision "$revision" '
  .format == "tondo-native-aot-performance/1"
  and .status == "passed"
  and .source_revision == $revision
  and .phase == "NATIVE-AOT-PERF-001"
  and .target == "x86_64-unknown-linux-gnu"
  and .profile == "release"
  and .protocol.minimum_sample_count == 27
  and .protocol.fresh_processes == true
  and .protocol.isolated_builds == true
  and .comparison.semantic_equivalence == "validated-before-measurement"
  and ([.candidates[] | select(.status == "passed" and (.build_samples | length == 27) and (.runtime_samples | length == 27))] | length == 2)
  and all(.candidates[]; .product.reproducible_builds == true)
' "$performance_report" >/dev/null || die "native AOT performance evidence failed N1"

jq -e '
  .format == "tondo-native-conf-evidence/1"
  and .status == "passed"
  and .mir == "tondo-mir-backend/1"
  and .oracle == "bytecode-vm-oracle"
  and .categories == {
      language:{status:"passed",cases:3},
      testing:{status:"passed",cases:3},
      stdlib:{status:"passed",cases:3}
  }
  and .backends.cranelift.status == "passed"
  and .backends.llvm.status == "passed"
  and .dimensions.cross_backend == true
  and .dimensions.independent_oracle == true
  and .dimensions.fail_closed == true
  and .physical_paths == []
' "$conf_report" >/dev/null || die "native conformance evidence failed N1"

jq -e '
  .format == "tondo-native-diff-evidence/1"
  and .status == "passed"
  and .oracle == "bytecode-vm-oracle"
  and .cases == 9
  and .properties.cross_backend_equality == true
  and .properties.native_equals_oracle == true
  and .properties.fail_closed_mismatch == true
  and .physical_paths == []
  and .divergences == []
' "$diff_report" >/dev/null || die "native differential evidence failed N1"

jq -e '
  .format == "tondo-native-rel-evidence/1"
  and .status == "passed"
  and .backend_selection == "cranelift"
  and .reproducible == true
  and .builds == 2
  and .promotion == "pending-gate-n1"
  and .physical_paths == []
  and .timestamps == false
  and .divergences == []
' "$rel_report" >/dev/null || die "native release evidence failed N1"

check_target() {
    local file="$1" id="$2" triple="$3" promotion="$4"
    jq -e --arg id "$id" --arg triple "$triple" '
      .format == "tondo-native-target-evidence/1"
      and .status == "passed"
      and .task == (if $id == "tondo-native-linux-x86-64-release" then "NATIVE-TARGET-001" else "NATIVE-TARGET-002" end)
      and .target == $id
      and .triple == $triple
      and .object_format == "elf"
      and .profile == "release"
      and .physical_smoke == true
      and .cross_compile_is_smoke == false
      and .product.status == "passed"
      and .physical_paths == []
    ' "$file" >/dev/null || die "target evidence failed N1: $id"
    local source_revision
    source_revision="$(jq -r '.source_revision // empty' "$file")"
    [[ "$source_revision" == "$revision" ]] || die "target evidence is stale: $id"
    [[ "$promotion" == "promoted" || "$promotion" == "candidate-smoke-only" ]] \
        || die "invalid target promotion class: $id"
}

check_target "$x86_report" "tondo-native-linux-x86-64-release" \
    "x86_64-unknown-linux-gnu" "promoted"
check_target "$arm_evidence" "tondo-native-linux-aarch64-release" \
    "aarch64-unknown-linux-gnu" "candidate-smoke-only"

! grep -Fq "$root" "$report" || die "N1 report leaked a physical workspace path"
echo "native N1: OK (Cranelift promoted for x86_64 GNU; ARM64 retained as candidate smoke)"
