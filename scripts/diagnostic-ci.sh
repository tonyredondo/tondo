#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

profile="${TONDO_DIAGNOSTIC_PROFILE:-all}"
fuzz_mode="${TONDO_DIAGNOSTIC_FUZZ_MODE:-smoke}"
if [[ "${1:-}" == "--profile" ]]; then
    profile="${2:?--profile requires race, leaks, crash or all}"
elif [[ -n "${1:-}" ]]; then
    echo "diagnostic CI: usage: $0 [--profile race|leaks|crash|all]" >&2
    exit 2
fi

case "$profile" in
    race|leaks|crash) profiles=("$profile") ;;
    all) profiles=(race leaks crash) ;;
    *) echo "diagnostic CI: unknown profile: $profile" >&2; exit 2 ;;
esac

evidence="$root/target/reliability/diagnostics-ci"
mkdir -p "$evidence/logs"

baseline_hash="$(sha256sum testing/quality-baseline.json | awk '{print $1}')"
contract_hash="$(sha256sum testing/diagnostic-ci.json | awk '{print $1}')"
lane_results='[]'
corpus_file_count="$(find fuzz/corpus/diagnostics -type f -printf '%P\n' | sort | wc -l | tr -d ' ')"
corpus_hash="$(find fuzz/corpus/diagnostics -type f -print0 \
    | sort -z \
    | xargs -0 sha256sum \
    | sha256sum \
    | awk '{print $1}')"

run_step() {
    local name="$1"
    shift
    echo "::group::$name"
    "$@" 2>&1 | tee "$evidence/logs/$name.log"
    echo "::endgroup::"
}

record_case() {
    local lane="$1"
    local kind="$2"
    local test_name="$3"
    local module="$4"
    local log_name="${lane}-${kind}-${test_name}"
    local started_ms="$(date +%s%3N)"

    # A separate cargo invocation is intentional: every diagnostic test case
    # gets a fresh process and therefore cannot inherit collector, scheduler,
    # roots, retry or artifact state from a neighboring case.
    run_step "$log_name" timeout --signal=TERM 120s \
        cargo test -p tondo-vm --lib --locked \
        "runtime::${module}::tests::${test_name}" -- --exact

    local elapsed_ms=$(( $(date +%s%3N) - started_ms ))
    lane_results="$(jq -c \
        --arg lane "$lane" \
        --arg kind "$kind" \
        --arg test "$test_name" \
        --argjson duration_ms "$elapsed_ms" \
        '. + [{lane:$lane,kind:$kind,test:$test,fresh_process:true,status:"passed",duration_ms:$duration_ms}]' \
        <<<"$lane_results")"
}

run_step contract scripts/diagnostic-ci-check.sh
run_step contract-tests scripts/diagnostic-ci-test.sh
run_step runner-contract scripts/diagnostic-test-check.sh
run_step runner-contract-tests scripts/diagnostic-test-test.sh

for lane in "${profiles[@]}"; do
    case "$lane" in
        race) module="race" ;;
        leaks) module="leak" ;;
        crash) module="dump" ;;
    esac

    lane_started_ms="$(date +%s%3N)"
    while IFS=$'\t' read -r kind test_name; do
        [[ -n "$test_name" ]] || continue
        record_case "$lane" "$kind" "$test_name" "$module"
    done < <(jq -r --arg lane "$lane" \
        '.lanes[] | select(.id == $lane) |
         ((.positive_tests | map(["positive", .])) +
          (.negative_tests | map(["negative", .])))[] |
         @tsv' testing/diagnostic-ci.json)
    lane_elapsed_ms=$(( $(date +%s%3N) - lane_started_ms ))
    lane_budget_seconds="$(jq -r --arg lane "$lane" '.lanes[] | select(.id == $lane) | .budget.max_wall_time_seconds' testing/diagnostic-ci.json)"
    (( lane_elapsed_ms <= lane_budget_seconds * 1000 )) || {
        echo "diagnostic CI: $lane lane exceeded ${lane_budget_seconds}s budget" >&2
        exit 1
    }
done

run_step cli-diagnostics cargo test -p tondo-cli --test cli --locked diagnostics_

if [[ "$fuzz_mode" != "skip" ]]; then
    case "$fuzz_mode" in
        smoke) run_step fuzz scripts/diagnostic-fuzz.sh --smoke ;;
        campaign) run_step fuzz scripts/diagnostic-fuzz.sh --campaign ;;
        *) echo "diagnostic CI: unknown fuzz mode: $fuzz_mode" >&2; exit 2 ;;
    esac
fi

after_baseline_hash="$(sha256sum testing/quality-baseline.json | awk '{print $1}')"
[[ "$baseline_hash" == "$after_baseline_hash" ]] || {
    echo "diagnostic CI: normal quality baseline changed during the run" >&2
    exit 1
}

case "$fuzz_mode" in
    smoke)
        fuzz_runs="${TONDO_DIAGNOSTIC_FUZZ_RUNS:-128}"
        fuzz_seed="${TONDO_DIAGNOSTIC_FUZZ_SEED:-4001}"
        fuzz_seconds=null
        ;;
    campaign)
        fuzz_runs=null
        fuzz_seed="${TONDO_DIAGNOSTIC_FUZZ_SEED:-5001}"
        fuzz_seconds="${TONDO_DIAGNOSTIC_FUZZ_SECONDS:-180}"
        ;;
    skip)
        fuzz_runs=null
        fuzz_seed=null
        fuzz_seconds=null
        ;;
esac

run_id="diag-ci-$(git rev-parse --short=12 HEAD)-${profile}"
jq -n \
    --arg format "tondo-diagnostic-ci-report/1" \
    --arg run_id "$run_id" \
    --arg profile "$profile" \
    --arg fuzz_mode "$fuzz_mode" \
    --arg source_revision "$(git rev-parse HEAD)" \
    --arg toolchain "$(rustc --version)" \
    --arg cargo_fuzz "$(cargo fuzz --version 2>/dev/null || printf 'unavailable')" \
    --arg fuzz_toolchain "${TONDO_DIAGNOSTIC_FUZZ_NIGHTLY:-nightly-2026-07-28}" \
    --arg corpus_hash "$corpus_hash" \
    --arg baseline_hash "$baseline_hash" \
    --arg baseline_after_hash "$after_baseline_hash" \
    --arg contract_hash "$contract_hash" \
    --argjson corpus_file_count "$corpus_file_count" \
    --argjson fuzz_runs "$fuzz_runs" \
    --argjson fuzz_seed "$fuzz_seed" \
    --argjson fuzz_seconds "$fuzz_seconds" \
    --argjson lane_budgets "$(jq -c '.lanes | map({profile:.id,budget:.budget})' testing/diagnostic-ci.json)" \
    --argjson report_limits "$(jq -c '{max_report_bytes:.budgets.max_report_bytes,max_dump_bytes:.budgets.max_dump_bytes,max_attempts:.budgets.max_attempts}' testing/diagnostic-ci.json)" \
    --argjson profiles "$(printf '%s\n' "${profiles[@]}" | jq -Rsc 'split("\n") | map(select(length > 0))')" \
    --argjson lane_results "$lane_results" \
    '{format:$format,run_id:$run_id,profile:$profile,profiles:$profiles,status:"passed",source_revision:$source_revision,toolchain:$toolchain,fuzz_toolchain:$fuzz_toolchain,cargo_fuzz:$cargo_fuzz,contract_sha256:$contract_hash,corpus:{file_count:$corpus_file_count,sha256:$corpus_hash},fuzz:{mode:$fuzz_mode,runs:$fuzz_runs,seed:$fuzz_seed,max_total_time_seconds:$fuzz_seconds},budgets:{measurement:"fresh-process-case-duration",lane_limits:$lane_budgets,report_limits:$report_limits,normal_baseline_sha256:$baseline_hash,normal_baseline_unchanged:($baseline_hash == $baseline_after_hash)},cases:$lane_results,unsupported_is_failure:true,logs:"target/reliability/diagnostics-ci/logs"}' \
    > "$evidence/summary.json"

echo "diagnostic CI: OK profile=$profile fuzz=$fuzz_mode evidence=$evidence/summary.json"
