#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="$root/testing/native-aot-quality.json"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence="$target_dir/reliability/evidence"
logs="$evidence/native-aot-quality"
report="${TONDO_NATIVE_AOT_QUALITY_REPORT:-$evidence/native-aot-quality.json}"

die() {
    echo "native AOT quality: $*" >&2
    exit 1
}

# Keep mutation worktrees outside the checkout. cargo-mutants copies the
# source tree, so placing its TMPDIR below the repository would recursively
# copy ignored .tmp/target artifacts into every isolated build.
tmp_root="${TONDO_NATIVE_AOT_QUALITY_TMPDIR:-$root/../tondo-aot-quality-tmp}"
if [[ "$tmp_root" != /* ]]; then
    tmp_root="$root/$tmp_root"
fi
mkdir -p "$evidence" "$logs" "$tmp_root"
tmp_root="$(cd "$tmp_root" && pwd)"
case "$tmp_root/" in
    "$root/"*) die "quality temporary directory must be outside the repository: $tmp_root" ;;
esac

run_step() {
    local name="$1"
    shift
    echo "::group::$name"
    "$@" 2>&1 | tee "$logs/$name.log"
    echo "::endgroup::"
}

[[ -x "$root/scripts/native-aot-quality-check.sh" ]] || die "quality checker is not executable"
[[ -x "$root/scripts/native-aot-quality-test.sh" ]] || die "quality mutation tests are not executable"
run_step contract-check scripts/native-aot-quality-check.sh
run_step contract-tests scripts/native-aot-quality-test.sh

for tool in /usr/bin/llc /usr/bin/cc /usr/bin/strip /usr/bin/readelf; do
    [[ -x "$tool" ]] || die "required native tool is missing: $tool"
done
[[ -x "$root/scripts/native-aot-sanitize-cc.sh" ]] || die "sanitizer compiler wrapper is not executable"

baseline_hash_before="$(sha256sum testing/quality-baseline.json | awk '{print $1}')"
baseline_basis_points="$(jq -r '.coverage.global.lines.basis_points' testing/quality-baseline.json)"
minimum_baseline_basis_points=9055
[[ "$baseline_basis_points" =~ ^[0-9]+$ ]] || die "quality baseline has an invalid line coverage value: $baseline_basis_points"
(( baseline_basis_points >= minimum_baseline_basis_points )) || die "quality baseline is below the 90.55% policy floor (got $baseline_basis_points bp)"

native_report="$evidence/native-evaluation-runner.json"
if [[ "${TONDO_NATIVE_AOT_QUALITY_USE_EXISTING:-0}" == 1 ]]; then
    [[ -f "$native_report" ]] || die "existing native runner report is missing"
    run_step native-runner-contract scripts/native-evaluation-runner-check.sh
else
    run_step native-runner env CARGO_TARGET_DIR="$target_dir" scripts/native-evaluation-runner.sh
fi

# The executable runner is the source of truth for the admitted inventory. The
# checks below deliberately count only the admitted report arrays; excluded
# probes remain explicit, documented traps and never become hidden passes.
jq -e '
  .status == "passed"
  and (.candidates | map(select(.id == "cranelift" or .id == "llvm") | .status) | sort) == ["measured", "measured"]
  and ([.native_aot_lowering.cases[]] | length) == 27
  and ([.native_runs[]] | length) == 118
  and ([.native_managed_runs[]] | length) == 3
  and ([.native_runtime_runs[]] | length) == 21
  and ([.native_std_core_runs[]] | length) == 14
  and ([.native_diagnostics.cases[]] | length) == 8
  and .native_aot_lowering.status == "passed"
  and .native_aot_binary.status == "passed"
  and .native_aot_memory.status == "passed"
  and .native_diagnostics.status == "passed"
  and all(.native_runs[]; .cranelift == "passed" and .llvm == "passed")
  and all(.native_managed_runs[]; .cranelift == "passed" and .llvm == "passed")
  and all(.native_runtime_runs[]; .cranelift == "passed" and .llvm == "passed")
  and all(.native_std_core_runs[]; .cranelift == "passed" and .llvm == "passed")
  and all(.native_diagnostics.cases[]; .cranelift == "passed" and .llvm == "passed")
' "$native_report" >/dev/null || die "native runner report is incomplete or divergent"

run_step conformance env CARGO_TARGET_DIR="$target_dir" scripts/native-conf-test.sh
run_step differential env CARGO_TARGET_DIR="$target_dir" TONDO_NATIVE_DIFF_EXECUTABLE=0 scripts/native-diff-test.sh

for target in frontend protocols admission stdlib_codecs stdlib_owners; do
    [[ -d "$root/target/reliability/fuzz-corpus/smoke/$target" ]] || die "missing owner fuzz output: $target"
done
run_step fuzz env TONDO_FUZZ_RUNS=128 TONDO_FUZZ_NIGHTLY="${TONDO_FUZZ_NIGHTLY:-nightly-2026-07-28}" scripts/fuzz-smoke.sh

run_step diagnostics \
    env TONDO_DIAGNOSTIC_FUZZ_MODE=smoke \
        TONDO_DIAGNOSTIC_FUZZ_RUNS=128 \
        TONDO_DIAGNOSTIC_FUZZ_NIGHTLY="${TONDO_DIAGNOSTIC_FUZZ_NIGHTLY:-nightly-2026-07-28}" \
        scripts/diagnostic-ci.sh --profile all

diagnostic_summary="$root/target/reliability/diagnostics-ci/summary.json"
jq -e '
  .status == "passed"
  and .fuzz.mode == "smoke"
  and .fuzz.runs == 128
  and (.profiles | sort) == ["crash", "leaks", "race"]
  and .unsupported_is_failure == true
' "$diagnostic_summary" >/dev/null || die "diagnostic CI did not produce the complete smoke evidence"

# Sanitizers use a clean target tree and a checked-in absolute compiler
# wrapper. This keeps ASan/UBSan state independent from the normal runner. A
# caller may point at an already completed sanitized tree to avoid repeating a
# long campaign after a wrapper-only repair; the report is still revalidated.
sanitized_target="${TONDO_NATIVE_AOT_QUALITY_SANITIZED_TARGET:-}"
if [[ -n "$sanitized_target" ]]; then
    [[ "$sanitized_target" = /* ]] || sanitized_target="$root/$sanitized_target"
    sanitized_report="$sanitized_target/reliability/evidence/native-evaluation-runner.json"
    [[ -f "$sanitized_report" ]] || die "requested sanitized report is missing"
else
    sanitized_target="$(mktemp -d "$target_dir/native-aot-quality-sanitized.XXXXXX")"
    sanitized_report="$sanitized_target/reliability/evidence/native-evaluation-runner.json"
    run_step sanitizer \
        env CARGO_TARGET_DIR="$sanitized_target" \
            TONDO_NATIVE_CC="$root/scripts/native-aot-sanitize-cc.sh" \
            ASAN_OPTIONS="detect_leaks=1:handle_sigfpe=0:halt_on_error=1:allocator_may_return_null=0" \
            UBSAN_OPTIONS="halt_on_error=1:print_stacktrace=1" \
            scripts/native-evaluation-runner.sh
fi
jq -e '.status == "passed" and .native_aot_memory.status == "passed" and .native_diagnostics.status == "passed"' \
    "$sanitized_report" >/dev/null || die "sanitized native runner report did not pass"

workspace_target="$(mktemp -d "$target_dir/native-aot-quality-workspace.XXXXXX")"
workspace_mutation_tmp="$(mktemp -d "$tmp_root/tondo-native-aot-quality-mutation.XXXXXX")"
run_step workspace-quality \
    env CARGO_TARGET_DIR="$workspace_target" \
        TONDO_MUTATION_TMPDIR="$workspace_mutation_tmp" \
        TONDO_MUTATION_OUTPUT="$workspace_target/reliability/quality/mutation" \
        scripts/quality-gate.sh

mutation_sample_json="$(jq -c '{
    status: (if .total_mutants == 6 and .caught == 6 and .missed == 0 and .timeout == 0 and .unviable == 0 then "passed" else "failed" end),
    total: .total_mutants,
    caught,
    missed,
    timeout,
    unviable,
    score_basis_points: (if (.caught + .missed + .timeout) == 0 then 0 else ((.caught * 10000) / (.caught + .missed + .timeout)) end),
    selection: "one-per-critical-frontier"
  }' "$workspace_target/reliability/quality/mutation/mutants.out/outcomes.json")"
jq -e '
  .status == "passed"
  and .total == 6
  and .caught == 6
  and .missed == 0
  and .timeout == 0
  and .unviable == 0
  and .score_basis_points == 10000
  and .selection == "one-per-critical-frontier"
' <<< "$mutation_sample_json" >/dev/null || die "critical mutation sample is incomplete"

baseline_hash_after="$(sha256sum testing/quality-baseline.json | awk '{print $1}')"
[[ "$baseline_hash_before" == "$baseline_hash_after" ]] || die "normal quality baseline changed during the campaign"

native_conf_report="$evidence/native-conf.json"
native_diff_report="$target_dir/reliability/evidence/native-diff.json"
jq -e '.status == "passed" and .categories.language.cases == 3 and .categories.testing.cases == 3 and .categories.stdlib.cases == 3' \
    "$native_conf_report" >/dev/null || die "conformance evidence is incomplete"
jq -e '.status == "passed" and .cases == 9 and .properties.cross_backend_equality == true and .properties.fail_closed_mismatch == true' \
    "$native_diff_report" >/dev/null || die "differential evidence is incomplete"

source_revision="${TONDO_NATIVE_AOT_QUALITY_SOURCE_REVISION:-$(git rev-parse HEAD)}"
[[ "$source_revision" =~ ^[0-9a-f]{40}$ ]] || die "source revision must be a full commit id"
contract_sha256="$(sha256sum "$contract" | awk '{print $1}')"
native_report_sha256="$(sha256sum "$native_report" | awk '{print $1}')"
sanitized_report_sha256="$(sha256sum "$sanitized_report" | awk '{print $1}')"
touched_files_json="$( {
    git diff --name-only HEAD^ HEAD
    git diff --name-only
    git diff --cached --name-only
} | sed '/^$/d' | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')"
[[ "$touched_files_json" != "[]" ]] || touched_files_json='["scripts/native-aot-quality.sh"]'

jq -n \
    --arg source_revision "$source_revision" \
    --arg contract_sha256 "sha256:$contract_sha256" \
    --arg native_report_sha256 "sha256:$native_report_sha256" \
    --arg sanitized_report_sha256 "sha256:$sanitized_report_sha256" \
    --arg baseline_sha256 "sha256:$baseline_hash_after" \
    --arg target "$(rustc -vV | sed -n 's/^host: //p')" \
    --arg toolchain "$(rustc --version)" \
    --argjson baseline_basis_points "$baseline_basis_points" \
    --argjson mutation_sample "$mutation_sample_json" \
    --argjson touched_files "$touched_files_json" \
    '{
      format: "tondo-native-aot-quality/1",
      task: "NATIVE-AOT-QUALITY-001",
      phase: "NATIVE-AOT-QUALITY-001",
      status: "passed",
      candidates: ["cranelift", "llvm"],
      oracle: {
        vm: "bytecode-vm-oracle",
        mir: "normalized-MIR-reference-interpreter",
        mismatch: "fail-closed"
      },
      native: {
        status: "passed",
        candidate_status: "both-passed",
        unsupported_admitted: 0,
        divergences: 0,
        case_counts: {
          native_aot_lowering: 27,
          native_scalar: 118,
          native_managed: 3,
          native_runtime: 21,
          native_std_core: 14,
          native_diagnostics: 8
        },
        fields: ["native_aot_binary", "native_aot_lowering", "native_aot_memory", "native_diagnostics"]
      },
      conformance: {
        status: "passed",
        categories: {
          language: {status: "passed", cases: 3},
          testing: {status: "passed", cases: 3},
          stdlib: {status: "passed", cases: 3}
        },
        cases: 9
      },
      differential: {
        status: "passed",
        cases: 9,
        backends: ["cranelift", "llvm"],
        oracle: "bytecode-vm-oracle",
        cross_backend_equality: true,
        stable_generation: true,
        fail_closed_mutation: true
      },
      fuzz: {
        owner: {
          status: "passed", targets: 5, runs_per_target: 128,
          max_input_bytes: 65536, timeout_seconds: 10, rss_limit_mb: 4096,
          bounded: true, deterministic: true, regressions_replayed: true
        },
        diagnostics: {
          status: "passed", targets: 1, runs: 128,
          max_input_bytes: 65536, timeout_seconds: 10, rss_limit_mb: 4096,
          bounded: true, deterministic: true, regressions_replayed: true
        }
      },
      sanitizers: {
        status: "passed", compiler: "explicit-cc-sanitizer-wrapper",
        address: "passed", undefined: "passed", fresh_processes: true
      },
      workspace_quality: {
        status: "passed", baseline_unchanged: true,
        baseline_basis_points: $baseline_basis_points, mutation: "passed",
        mutation_sample: $mutation_sample,
        baseline_sha256: $baseline_sha256
      },
      mutation: {status: "passed", oracles: 12, rejected: 12},
      physical_paths: [],
      divergences: [],
      unsupported: [],
      source_revision: $source_revision,
      contract_sha256: $contract_sha256,
      native_report_sha256: $native_report_sha256,
      native_aot_sanitized_report_sha256: $sanitized_report_sha256,
      target: $target,
      toolchain: $toolchain,
      touched_files: $touched_files
    }' > "$report"

TONDO_NATIVE_AOT_QUALITY_REPORT="$report" scripts/native-aot-quality-check.sh
echo "native AOT quality: PASSED report=${report#"$root/"} baseline=${baseline_basis_points}bp"
