#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_ASYNC_SELECT_CONTRACT:-$root/testing/async-select-conformance.json}"
result="${TONDO_ASYNC_SELECT_RESULT:-$root/target/reliability/evidence/conformance-result.json}"
evidence_dir="${TONDO_ASYNC_SELECT_EVIDENCE_DIR:-$root/target/reliability/evidence}"
report="$evidence_dir/async-select-conformance.json"

die() {
    echo "async select conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root/"}"
[[ -f "$result" ]] || die "missing composed conformance result: ${result#"$root/"}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null && die "contract contains CR or trailing whitespace"

jq -e '
    .format == "tondo-async-select-conformance/1"
    and .edition == "0.1"
    and .task == "ASYNC-SELECT-VM-CONF-001"
    and .owner == "tondo-vm"
    and .status == "closed"
    and .lineage == "tondo-draft"
    and .suite == "tondo-conformance-draft"
    and .result_format == "tondo-conformance-result-draft/2"
    and .manifest == "conformance/0.1/manifest.json"
    and (.manifest_sha256 | test("^[0-9a-f]{64}$"))
    and .target == "tondo-vm-hosted"
    and .profile == "hosted"
    and .pipeline == ["parser", "formatter", "interface", "bytecode", "verified-bytecode", "vm"]
    and .oracle == "exact-observation-hashes"
    and .full_suite_case_count == 206
    and ([.cases[].id] == [
        "concurrency/select-join-loser-runtime",
        "concurrency/select-join-runtime",
        "concurrency/select-runtime"
    ])
    and all(.cases[];
        .fixture | test("^conformance/0\\.1/cases/concurrency/select-[a-z-]+\\.to$")
    )
    and all(.cases[];
        (.fixture_sha256 | test("^[0-9a-f]{64}$"))
        and .group == "concurrency"
        and .repetitions == 32
        and (.observation_sha256 | test("^[0-9a-f]{64}$"))
    )
    and .invariants == {
        "full_suite_passed": true,
        "selected_cases_are_concurrency": true,
        "repetitions_are_exact": true,
        "observations_are_identical_per_case": true,
        "all_selected_observations_match_expected": true,
        "native_backend_claim": false
    }
    and .report == "target/reliability/evidence/async-select-conformance.json"
    and .next_blocks == ["STD-A-PERF-001", "DIAG-SPEC-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

manifest_sha256="$(sha256sum conformance/0.1/manifest.json | cut -d' ' -f1)"
contract_manifest_sha256="$(jq -r '.manifest_sha256' "$contract")"
[[ "$manifest_sha256" == "$contract_manifest_sha256" ]] || die "live suite manifest hash differs from the contract"

while IFS=$'\t' read -r fixture expected_sha; do
    [[ -n "$fixture" && -n "$expected_sha" ]] || die "empty fixture record"
    [[ -f "$root/$fixture" ]] || die "missing fixture: $fixture"
    actual_sha="$(sha256sum "$root/$fixture" | cut -d' ' -f1)"
    [[ "$actual_sha" == "$expected_sha" ]] || die "fixture hash mismatch: $fixture"
done < <(jq -r '.cases[] | [.fixture, .fixture_sha256] | @tsv' "$contract")

jq -e \
    --arg format "tondo-conformance-result-draft/2" \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg target "tondo-vm-hosted" \
    --arg lineage "tondo-draft" \
    --arg suite "tondo-conformance-draft" \
    --argjson full_suite_case_count "$(jq -r '.full_suite_case_count' "$contract")" \
    '.format == $format
     and .suite == $suite
     and .edition == "0.1"
     and .manifest_sha256 == $manifest_sha256
     and .lineage == $lineage
     and .target.name == $target
     and .target.profile == "hosted"
     and .passed == true
     and (.cases | length) == $full_suite_case_count
     and all(.cases[]; .repetitions > 0 and (.observation_sha256 | length) == .repetitions)
     and any(.case_layers[]; .id == "async-select" and (.cases | length) == 2)
    ' "$result" >/dev/null || die "composed result does not prove the complete hosted suite"

while IFS=$'\t' read -r id group repetitions expected_observation; do
    jq -e \
        --arg id "$id" \
        --arg group "$group" \
        --arg expected "$expected_observation" \
        --argjson repetitions "$repetitions" \
        '[.cases[] | select(.id == $id)] as $rows
         | ($rows | length) == 1
         and $rows[0].group == $group
         and $rows[0].repetitions == $repetitions
         and (($rows[0].observation_sha256 | unique) == [$expected])
         and all($rows[0].observation_sha256[]; . == $expected)
        ' "$result" >/dev/null || die "exact observation mismatch: $id"
done < <(jq -r '.cases[] | [.id, .group, .repetitions, .observation_sha256] | @tsv' "$contract")

mkdir -p "$evidence_dir"
revision="$(git rev-parse HEAD)"
result_sha256="$(sha256sum "$result" | cut -d' ' -f1)"
jq -n \
    --slurpfile contract "$contract" \
    --slurpfile result "$result" \
    --arg revision "$revision" \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg result_sha256 "$result_sha256" \
    '{
        format: "tondo-async-select-conformance/1",
        task: $contract[0].task,
        revision: $revision,
        manifest_sha256: $manifest_sha256,
        result_sha256: $result_sha256,
        suite_case_count: ($result[0].cases | length),
        selected_cases: [
            $contract[0].cases[] as $expected
            | ($result[0].cases[] | select(.id == $expected.id)) as $actual
            | {
                id: $actual.id,
                repetitions: $actual.repetitions,
                observation_sha256: ($actual.observation_sha256 | unique[0])
            }
        ],
        pipeline: $contract[0].pipeline,
        native_backend_claim: false,
        status: "passed"
    }' > "$report"

echo "async select conformance: OK (206/206 cases; 3 selected cases; 32 exact observations each; report: ${report#"$root/"})"
