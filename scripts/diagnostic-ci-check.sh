#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_DIAGNOSTIC_CI_CONTRACT:-$root/testing/diagnostic-ci.json}"

die() {
    echo "diagnostic CI: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing CI registry: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "registry must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "registry contains CR or trailing whitespace"

jq -e '
  .format == "tondo-diagnostic-ci/1"
  and .owner == "toolchain.diagnostics"
  and .edition == "0.1"
  and .phase == "DIAG-CI-001"
  and .status == "implemented"
  and .contract == "docs/contracts/diagnostic-ci.md"
  and .runner_contract == "testing/diagnostic-test.json"
  and .profiles == ["race", "leaks", "crash"]
  and .all == {expands_to: ["race", "leaks", "crash"], order: ["race", "leaks", "crash"]}
  and ([.lanes[].id] == ["race", "leaks", "crash"])
  and all(.lanes[];
      .profile == .id
      and .package == "tondo-vm"
      and (.positive_tests | type == "array" and length > 0 and (unique | length == length))
      and (.negative_tests | type == "array" and length > 0 and (unique | length == length))
      and (.budget.max_events == 1000000)
      and (.budget.max_report_bytes == 16777216)
      and (.budget.max_wall_time_seconds > 0)
      and (.budget.max_overhead_basis_points > 0)
  )
  and .corpus == {
    root: "fuzz/corpus/diagnostics",
    positive_root: "fuzz/corpus/diagnostics/positive",
    negative_root: "fuzz/corpus/diagnostics/negative",
    regression_root: "fuzz/corpus/diagnostics/regressions",
    required_profiles: ["race", "leaks", "crash"],
    cases: {
      race: {positive: ["race-conflict"], negative: ["race-clean"]},
      leaks: {positive: ["leak-growth"], negative: ["leak-clean"]},
      crash: {positive: ["crash-dump"], negative: ["crash-clean"]}
    },
    persistent_minimized_inputs: true,
    replay_command: "cargo +nightly-2026-07-28 fuzz run diagnostics fuzz/corpus/diagnostics -- -runs=0"
  }
  and .fuzz.target == "diagnostics"
  and .fuzz.script == "scripts/diagnostic-fuzz.sh"
  and .fuzz.toolchain == "nightly-2026-07-28"
  and .fuzz.cargo_fuzz == "0.13.2"
  and .fuzz.max_input_bytes == 65536
  and .fuzz.timeout_seconds == 10
  and .fuzz.rss_limit_mb == 4096
  and .fuzz.smoke == {runs: 128, seed: 4001}
  and .fuzz.campaign == {max_total_time_seconds: 180, seed: 5001}
  and .budgets == {
    comparison: "same-fixture-uninstrumented-vs-profiled-process",
    time_unit: "milliseconds",
    memory_unit: "bytes",
    measurement: "median-of-three-fresh-processes",
    max_report_bytes: 16777216,
    max_dump_bytes: 268435456,
    max_attempts: 512,
    overhead_is_separate_from_normal_baseline: true,
    budget_failure_is_fatal: true
  }
  and .promotion.status == "promoted"
  and .promotion.workflow == ".github/workflows/diagnostics.yml"
  and ([.promotion.gates[].id] == ["contract", "corpus", "profiles", "fuzz", "budgets", "ci"])
  and .promotion.normal_baseline_unchanged == true
  and .promotion.unsupported_is_failure == true
  and .privacy == {
    payloads: "redacted-by-default",
    network_upload: false,
    physical_paths: "omitted",
    secrets: "never-emitted"
  }
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 18
  and .next_blocks == ["NATIVE-001"]
' "$contract" >/dev/null || die "invalid machine-readable CI contract"

for path in \
    docs/contracts/diagnostic-ci.md \
    testing/diagnostic-test.json \
    scripts/diagnostic-ci.sh \
    scripts/diagnostic-fuzz.sh \
    fuzz/fuzz_targets/diagnostics.rs \
    fuzz/Cargo.toml \
    .github/workflows/diagnostics.yml; do
    [[ -f "$root/$path" ]] || die "missing CI evidence: $path"
done

[[ -x "$root/scripts/diagnostic-ci.sh" ]] || die "diagnostic CI runner is not executable"
[[ -x "$root/scripts/diagnostic-fuzz.sh" ]] || die "diagnostic fuzz runner is not executable"

for directory in \
    fuzz/corpus/diagnostics/positive \
    fuzz/corpus/diagnostics/negative \
    fuzz/corpus/diagnostics/regressions; do
    [[ -d "$root/$directory" ]] || die "missing corpus directory: $directory"
done

for profile in race leaks crash; do
    for side in positive negative; do
        root_path="$(jq -r --arg side "$side" '.corpus[($side + "_root")]' "$contract")"
        while IFS= read -r case_name; do
            [[ -n "$case_name" ]] || continue
            case_path="$root/$root_path/$case_name"
            [[ -s "$case_path" ]] || die "missing ${side} corpus case $profile: ${case_path#"$root"/}"
        done < <(jq -r --arg profile "$profile" --arg side "$side" '.corpus.cases[$profile][$side][]' "$contract")
    done
done

for profile in race leaks crash; do
    case "$profile" in
        race) source="$root/crates/tondo-vm/src/runtime/race.rs" ;;
        leaks) source="$root/crates/tondo-vm/src/runtime/leak.rs" ;;
        crash) source="$root/crates/tondo-vm/src/runtime/dump.rs" ;;
    esac
    while IFS= read -r test_name; do
        [[ -n "$test_name" ]] || continue
        grep -Fq "fn $test_name" "$source" || die "missing positive test $profile::$test_name"
    done < <(jq -r --arg profile "$profile" '.lanes[] | select(.id == $profile) | .positive_tests[]' "$contract")
    while IFS= read -r test_name; do
        [[ -n "$test_name" ]] || continue
        grep -Fq "fn $test_name" "$source" || die "missing negative test $profile::$test_name"
    done < <(jq -r --arg profile "$profile" '.lanes[] | select(.id == $profile) | .negative_tests[]' "$contract")
done

workflow="$root/.github/workflows/diagnostics.yml"
grep -Fq 'workflow_dispatch:' "$workflow" || die "diagnostic workflow is not opt-in"
! grep -Eq '^  push:' "$workflow" || die "diagnostic workflow runs on every push"
grep -Fq 'scripts/diagnostic-ci.sh' "$workflow" || die "workflow does not run the diagnostic gate"
grep -Fq 'TONDO_DIAGNOSTIC_PROFILE' "$workflow" || die "workflow does not select a profile"
! grep -Fq 'secrets.' "$workflow" || die "workflow references secrets"

runner="$root/scripts/diagnostic-ci.sh"
grep -Fq 'fresh process' "$runner" || die "runner does not document fresh-process isolation"
grep -Fq 'timeout --signal=TERM 120s' "$runner" || die "runner has no fail-closed lane timeout"
grep -Fq -- '-- --exact' "$runner" || die "runner does not isolate exact test cases"
grep -Fq 'lane_results' "$runner" || die "runner does not persist per-case evidence"
grep -Fq 'normal_baseline_unchanged' "$runner" || die "runner does not verify the normal baseline"
grep -Fq -- '-max_len=65536' "$root/scripts/diagnostic-fuzz.sh" || die "fuzz input bound is not enforced"

grep -Fq 'unsupported_is_failure' "$contract" || die "unsupported policy is absent"
grep -Fq 'normal_baseline_unchanged' "$contract" || die "normal baseline boundary is absent"
grep -Fq 'fresh-process' "$root/docs/contracts/diagnostic-ci.md" || die "fresh-process budget rule is absent"
grep -Fq 'regressions/' "$root/docs/contracts/diagnostic-ci.md" || die "persistent regression corpus is absent"

echo "diagnostic CI: OK (opt-in lanes, persistent corpus, bounded fuzzing, budgets and promotion gate)"
