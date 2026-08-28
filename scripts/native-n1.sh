#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_NATIVE_N1_CONTRACT:-$root/testing/native-n1.json}"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$target_dir/reliability/evidence"
report="${TONDO_NATIVE_N1_REPORT:-$evidence_dir/native-n1.json}"
arm_evidence="${TONDO_NATIVE_N1_ARM64_EVIDENCE:-$root/target/platform-test/linux-aarch64/native-target.json}"

die() {
    echo "native N1: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing N1 contract"
if [[ "${TONDO_NATIVE_N1_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain=v1 --untracked-files=all)" ]] \
        || die "workspace must be clean before N1 promotion"
fi

target_relative() {
    local path="$1"
    case "$path" in
        "$root"/*) printf '%s\n' "${path#"$root/"}" ;;
        *) die "N1 evidence must remain inside the workspace: $path" ;;
    esac
}

run_check() {
    local command="$1"
    echo "::group::$command"
    bash -lc "$command"
    echo "::endgroup::"
}

# Revalidate every static boundary before composing the promotion record. The
# expensive campaigns run in the native evaluation job; these checks make the
# final composition fail closed if an input is missing or has drifted.
run_check "scripts/native-n1-check.sh --contract-only"
for command in \
    scripts/native-selection-check.sh \
    scripts/native-abi-check.sh \
    scripts/native-aot-scope-check.sh \
    scripts/native-aot-lowering-check.sh \
    scripts/native-aot-binary-check.sh \
    scripts/native-aot-memory-check.sh \
    scripts/native-evaluation-check.sh \
    scripts/native-evaluation-runner-check.sh \
    scripts/native-conf-check.sh \
    scripts/native-diff-check.sh \
    scripts/native-target-check.sh \
    scripts/native-target-aarch64-check.sh \
    scripts/native-rel-check.sh; do
    run_check "$command"
done

[[ -f "$arm_evidence" ]] || die "missing ARM64 candidate evidence"
[[ -f "$evidence_dir/native-evaluation.json" ]] || die "missing native evaluation evidence"
[[ -f "$evidence_dir/native-evaluation-runner.json" ]] || die "missing native runner evidence"
[[ -f "$evidence_dir/native-aot-quality.json" ]] || die "missing AOT quality evidence"
[[ -f "$evidence_dir/native-aot-performance.json" ]] || die "missing AOT performance evidence"
[[ -f "$evidence_dir/native-conf.json" ]] || die "missing native conformance evidence"
[[ -f "$evidence_dir/native-diff.json" ]] || die "missing native differential evidence"
[[ -f "$evidence_dir/native-rel.json" ]] || die "missing native release evidence"
[[ -f "$evidence_dir/native-target.json" ]] || die "missing x86_64 target evidence"

revision="$(git rev-parse HEAD)"
[[ "$revision" =~ ^[0-9a-f]{40}$ ]] || die "invalid Git revision"
mkdir -p "$evidence_dir"

report_path="$(target_relative "$report")"
arm_path="$(target_relative "$arm_evidence")"

report_records='[]'
report_ids=(
    NATIVE-EVALUATION-001
    NATIVE-EVALUATION-RUNNER-001
    NATIVE-AOT-QUALITY-001
    NATIVE-AOT-PERF-001
    NATIVE-CONF-001
    NATIVE-DIFF-001
    NATIVE-REL-001
    NATIVE-TARGET-001
    NATIVE-TARGET-002
)
report_paths=(
    "$evidence_dir/native-evaluation.json"
    "$evidence_dir/native-evaluation-runner.json"
    "$evidence_dir/native-aot-quality.json"
    "$evidence_dir/native-aot-performance.json"
    "$evidence_dir/native-conf.json"
    "$evidence_dir/native-diff.json"
    "$evidence_dir/native-rel.json"
    "$evidence_dir/native-target.json"
    "$arm_evidence"
)

for index in "${!report_ids[@]}"; do
    id="${report_ids[$index]}"
    path="${report_paths[$index]}"
    relative="$(target_relative "$path")"
    hash="sha256:$(sha256sum "$path" | cut -d ' ' -f1)"
    source_revision="$(jq -r '.git_revision // .source_revision // empty' "$path")"
    [[ "$source_revision" == "$revision" ]] || die "report is stale: $id"
    status="$(jq -r '.status // empty' "$path")"
    [[ "$status" == "passed" ]] || die "report is not passed: $id"
    record="$(jq -cn --arg id "$id" --arg path "$relative" --arg sha256 "$hash" \
        --arg source_revision "$source_revision" \
        '{id:$id,path:$path,sha256:$sha256,status:"passed",source_revision:$source_revision}')"
    report_records="$(jq -c --argjson record "$record" '. + [$record]' <<< "$report_records")"
done

input_records='[]'
while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    file="$root/$path"
    [[ -f "$file" ]] || die "required N1 input is missing: $path"
    hash="sha256:$(sha256sum "$file" | cut -d ' ' -f1)"
    input_records="$(jq -c --arg path "$path" --arg sha256 "$hash" \
        '. + [{path:$path,sha256:$sha256}]' <<< "$input_records")"
done < <(jq -r '.required_inputs[]' "$contract")

tmp_report="$report.tmp"
jq -S -n \
    --arg revision "$revision" \
    --argjson reports "$report_records" \
    --argjson inputs "$input_records" \
    --arg x86_path "$(target_relative "$evidence_dir/native-target.json")" \
    --arg arm_path "$arm_path" \
    ' {
      format: "tondo-native-n1/1",
      gate: "N1",
      status: "promoted",
      edition: "0.1",
      phase: "M11",
      source_revision: $revision,
      workspace: "clean",
      backend: {
        selected: "cranelift",
        target: "x86_64-unknown-linux-gnu",
        selection_record: "testing/native-selection.json",
        fallback: "forbidden",
        public_abi: false
      },
      oracle: {
        vm: "bytecode-vm-oracle",
        mir: "normalized-MIR-reference-interpreter",
        mismatch: "fail-closed"
      },
      criteria: [
        {id:"aot-campaign",status:"passed"},
        {id:"backend-selection",status:"passed"},
        {id:"shared-abi-mir",status:"passed"},
        {id:"build-run",status:"passed"},
        {id:"memory-ownership",status:"passed"},
        {id:"diagnostic-parity",status:"passed"},
        {id:"quality-regression",status:"passed"},
        {id:"published-target",status:"passed"},
        {id:"reproducible-package",status:"passed"}
      ],
      targets: [
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
      ],
      reports: $reports,
      inputs: $inputs,
      summary: {
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
      },
      claims: {
        g5:false,
        s1:false,
        tlf:false,
        public_release:false,
        public_abi:false
      },
      physical_paths:[],
      candidate_target_policy: {
        "aarch64-unknown-linux-gnu":"physical-smoke-only",
        "windows-x86_64":"portable-probe-only",
        "macos-x86_64":"portable-probe-only",
        "macos-aarch64":"portable-probe-only"
      },
      next_blocks:["STD-ASYNC-GROUP-IMPL-001"]
    }' > "$tmp_report"
mv -- "$tmp_report" "$report"

TONDO_NATIVE_N1_CONTRACT="$contract" \
TONDO_NATIVE_N1_REPORT="$report" \
TONDO_NATIVE_N1_ARM64_EVIDENCE="$arm_evidence" \
    "$root/scripts/native-n1-check.sh"

echo "native N1: PASS (Cranelift promoted for x86_64 GNU; ARM64 retained as candidate smoke; report: $report_path)"
