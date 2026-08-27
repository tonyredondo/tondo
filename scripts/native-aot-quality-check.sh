#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_AOT_QUALITY_CONTRACT:-$root/testing/native-aot-quality.json}"
report="${TONDO_NATIVE_AOT_QUALITY_REPORT:-}"

die() {
    echo "native AOT quality: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract has CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-aot-quality/1"
  and .owner == "toolchain.native_evaluation"
  and .edition == "0.1"
  and .task == "NATIVE-AOT-QUALITY-001"
  and .phase == "NATIVE-AOT-QUALITY-001"
  and .status == "closed"
  and .contract == "docs/contracts/native-aot-quality.md"
  and .runner == "scripts/native-aot-quality.sh"
  and .checker == "scripts/native-aot-quality-check.sh"
  and .tests == "scripts/native-aot-quality-test.sh"
  and .report == "target/reliability/evidence/native-aot-quality.json"
  and .native_runner == "scripts/native-evaluation-runner.sh"
  and .native_report == "target/reliability/evidence/native-evaluation-runner.json"
  and .input == {
      mir: "tondo-mir-backend/1",
      target: "host-native-target-from-runner",
      profile: "release",
      runtime_abi: "tondo-runtime-draft/1",
      stdlib: "STD-0.1A",
      candidates: ["cranelift", "llvm"],
      oracle: "bytecode-vm-oracle-and-normalized-MIR-reference-interpreter",
      same_input: true,
      fresh_processes: true
  }
  and .corpus == {
      language: "testing/native-conf-language.json",
      testing: "testing/native-conf-testing.json",
      stdlib: "testing/native-conf-stdlib.json",
      differential: "testing/native-diff.json",
      diagnostics: "testing/diagnostic-ci.json",
      stdlib_fuzz: "testing/stdlib-fuzz.json",
      aot_lowering: "testing/native-aot-lowering.json",
      aot_binary: "testing/native-aot-binary.json",
      aot_memory: "testing/native-aot-memory.json"
  }
  and .protocol.native_aot_lowering_cases == 27
  and .protocol.native_scalar_cases == 118
  and .protocol.native_managed_cases == 3
  and .protocol.native_runtime_cases == 21
  and .protocol.native_std_core_cases == 14
  and .protocol.native_diagnostic_cases == 8
  and .protocol.conformance_cases == 9
  and .protocol.differential_cases == 9
  and .protocol.owner_fuzz_targets == 5
  and .protocol.diagnostic_fuzz_targets == 1
  and .protocol.fuzz_runs_per_target == 128
  and .protocol.fuzz_max_input_bytes == 65536
  and .protocol.fuzz_timeout_seconds == 10
  and .protocol.fuzz_rss_limit_mb == 4096
  and .protocol.sanitizers == ["address", "undefined"]
  and .protocol.normal_baseline_must_be_unchanged == true
  and .required_evidence.native_report_fields == [
      "native_aot_lowering", "native_aot_binary", "native_aot_memory",
      "native_diagnostics"
  ]
  and .required_evidence.candidate_status == "both-passed"
  and .required_evidence.unsupported_admitted == 0
  and .required_evidence.divergences == 0
  and .required_evidence.physical_paths == "forbidden"
  and .required_evidence.mutation == "all-critical-oracles-rejected"
  and (.mutation_oracles | length == 12 and unique_values)
  and (.invariants | length == 15 and unique_values)
  and (.negative_cases | length == 16 and unique_values)
  and .next_blocks == ["NATIVE-AOT-PERF-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    docs/contracts/native-aot-quality.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/tracker-graph.json \
    testing/native-aot-lowering.json \
    testing/native-aot-binary.json \
    testing/native-aot-memory.json \
    testing/native-conf-language.json \
    testing/native-conf-testing.json \
    testing/native-conf-stdlib.json \
    testing/native-diff.json \
    testing/diagnostic-ci.json \
    testing/stdlib-fuzz.json \
    tools/native-evaluation/src/main.rs; do
    [[ -f "$root/$path" ]] || die "missing quality input: $path"
done

for path in \
    scripts/native-aot-quality.sh \
    scripts/native-aot-quality-test.sh \
    scripts/native-evaluation-runner.sh \
    scripts/native-conf-test.sh \
    scripts/native-diff-test.sh \
    scripts/fuzz-smoke.sh \
    scripts/diagnostic-ci.sh \
    scripts/quality-gate.sh \
    scripts/native-aot-sanitize-cc.sh; do
    [[ -x "$root/$path" ]] || die "quality tool is not executable: $path"
done

grep -Fq 'NATIVE-AOT-QUALITY-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not reference the quality block"
grep -Fq 'NATIVE-AOT-PERF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not expose the next AOT block"
jq -e '
  (.task_dependencies["NATIVE-AOT-QUALITY-001"] | index("NATIVE-AOT-LOWER-001")) != null
  and (.task_dependencies["NATIVE-AOT-QUALITY-001"] | index("NATIVE-CONF-001")) != null
  and (.task_dependencies["NATIVE-AOT-QUALITY-001"] | index("NATIVE-DIFF-001")) != null
  and (.task_dependencies["NATIVE-AOT-QUALITY-001"] | index("DIAG-NATIVE-001")) != null
  and (.task_dependencies["NATIVE-AOT-PERF-001"] | index("NATIVE-AOT-QUALITY-001")) != null
' testing/tracker-graph.json >/dev/null \
    || die "tracker graph does not preserve quality evidence order"

for needle in \
    'native_aot_lowering' \
    'native_aot_binary' \
    'native_aot_memory' \
    'native_diagnostics' \
    'native_runs' \
    'native_std_core_runs' \
    'native_runtime_runs'; do
    grep -Fq "$needle" tools/native-evaluation/src/main.rs \
        || die "native runner does not expose required quality input: $needle"
done

if [[ -n "$report" ]]; then
    [[ -f "$report" ]] || die "quality report does not exist: $report"
    jq -e --arg root "$root" '
      .format == "tondo-native-aot-quality/1"
      and .task == "NATIVE-AOT-QUALITY-001"
      and .phase == "NATIVE-AOT-QUALITY-001"
      and .status == "passed"
      and .candidates == ["cranelift", "llvm"]
      and .oracle == {
          vm: "bytecode-vm-oracle",
          mir: "normalized-MIR-reference-interpreter",
          mismatch: "fail-closed"
      }
      and .native.status == "passed"
      and .native.candidate_status == "both-passed"
      and .native.unsupported_admitted == 0
      and .native.divergences == 0
      and .native.case_counts == {
          native_aot_lowering: 27,
          native_scalar: 118,
          native_managed: 3,
          native_runtime: 21,
          native_std_core: 14,
          native_diagnostics: 8
      }
      and (.native.fields | sort) == [
          "native_aot_binary", "native_aot_lowering", "native_aot_memory",
          "native_diagnostics"
      ]
      and .conformance.status == "passed"
      and .conformance.categories == {
          language: {status: "passed", cases: 3},
          testing: {status: "passed", cases: 3},
          stdlib: {status: "passed", cases: 3}
      }
      and .conformance.cases == 9
      and .differential == {
          status: "passed",
          cases: 9,
          backends: ["cranelift", "llvm"],
          oracle: "bytecode-vm-oracle",
          cross_backend_equality: true,
          stable_generation: true,
          fail_closed_mutation: true
      }
      and .fuzz.owner == {
          status: "passed",
          targets: 5,
          runs_per_target: 128,
          max_input_bytes: 65536,
          timeout_seconds: 10,
          rss_limit_mb: 4096,
          bounded: true,
          deterministic: true,
          regressions_replayed: true
      }
      and .fuzz.diagnostics == {
          status: "passed",
          targets: 1,
          runs: 128,
          max_input_bytes: 65536,
          timeout_seconds: 10,
          rss_limit_mb: 4096,
          bounded: true,
          deterministic: true,
          regressions_replayed: true
      }
      and .sanitizers == {
          status: "passed",
          compiler: "explicit-cc-sanitizer-wrapper",
          address: "passed",
          undefined: "passed",
          fresh_processes: true
      }
      and .workspace_quality.status == "passed"
      and .workspace_quality.baseline_unchanged == true
      and .workspace_quality.baseline_basis_points == 9055
      and .workspace_quality.mutation == "passed"
      and .mutation.status == "passed"
      and .mutation.oracles == 12
      and .mutation.rejected == 12
      and .physical_paths == []
      and .divergences == []
      and .unsupported == []
      and ((.source_revision | test("^[0-9a-f]{40}$")))
      and ((.contract_sha256 | test("^sha256:[0-9a-f]{64}$")))
      and ((.native_report_sha256 | test("^sha256:[0-9a-f]{64}$")))
      and ((.workspace_quality.baseline_sha256 | test("^sha256:[0-9a-f]{64}$")))
      and ((.native_aot_sanitized_report_sha256 | test("^sha256:[0-9a-f]{64}$")))
      and ((.touched_files | all(. | type == "string" and (contains("/") or . == "Cargo.lock"))))
      and ((. | tostring | contains($root)) | not)
    ' "$report" >/dev/null || die "quality report does not prove the complete AOT gate"
fi

echo "native AOT quality: OK (full differential, conformance, fuzz, sanitizer and fail-closed mutation contract)"
