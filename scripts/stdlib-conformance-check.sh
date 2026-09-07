#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CONFORMANCE_CONTRACT:-testing/stdlib-conformance.json}"
evidence="${TONDO_STDLIB_CONFORMANCE_EVIDENCE:-${CARGO_TARGET_DIR:-target}/reliability/evidence/stdlib-conformance.json}"

die() {
    echo "stdlib conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root/"}"
[[ $# == 0 || ($# == 1 && "$1" == --plan) ]] || die "usage: stdlib-conformance-check.sh [--plan]"

generated="$(mktemp "${TMPDIR:-/tmp}/tondo-stdlib-conformance-plan.XXXXXX")"
trap 'rm -f "$generated"' EXIT
scripts/stdlib-conformance-generate.sh "$generated" >/dev/null
cmp -s "$generated" "$contract" || die "case plan is stale or modified; regenerate with stdlib-conformance-generate.sh"

manifest_sha256="$(sha256sum conformance/0.1/manifest.json | cut -d' ' -f1)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
jq -e --arg manifest_sha256 "$manifest_sha256" \
    --slurpfile suite conformance/0.1/manifest.json '
    .format == "tondo-stdlib-conformance/1" and .edition == "0.1"
    and .phase == "STD-0.1A" and .status == "planned"
    and .rules.generation_does_not_promote
    and .runner.manifest_sha256 == $manifest_sha256
    and .runner.full_suite_case_count == ($suite[0].cases | length)
    and (.owners | length) == 22
    and ([.owners[].id] | unique | length) == 22
    and all(.owners[]; .status == "pending"
      and (.reason | type == "string" and length > 0)
      and (.owner_command | startswith("scripts/")) and (.cases | length) > 0)
    ' "$contract" >/dev/null || die "invalid case plan"
while IFS= read -r ref; do
    [[ -e "$root/${ref%%#*}" ]] || die "missing conformance reference: $ref"
done < <(jq -r '.owners[].refs[]' "$contract")
if [[ "${1:-}" == --plan ]]; then
    echo "stdlib conformance: consistent case plan; no execution or promotion claimed"
    exit 0
fi

[[ -f "$evidence" ]] || die "missing execution evidence: ${evidence#"$root/"}"
revision="$(git rev-parse HEAD)"
tree_sha256="$(cargo run -p tondo-reliability --locked -- quality provenance --root . | jq -r '.tree_sha256')"

jq -e \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg contract_sha256 "$contract_sha256" \
    --arg revision "$revision" \
    --arg tree_sha256 "$tree_sha256" \
    --slurpfile contract "$contract" \
    '
      . as $observation
      | ($contract[0]) as $contract
      | .format == "tondo-stdlib-conformance-evidence/1"
      and .edition == "0.1"
      and .phase == "STD-0.1A"
      and .status == "passed"
      and .scope == "declared-cases"
      and .public_row_coverage == "unverified"
      and .promotion == "pending"
      and .revision == $revision
      and .tree_sha256 == $tree_sha256
      and .contract_sha256 == $contract_sha256
      and .manifest_sha256 == $manifest_sha256
      and .full_suite.passed == true
      and .full_suite.cases == $contract.runner.full_suite_case_count
      and (.commands | type == "array" and length > 0 and all(.[]; .status == "passed" and (.log_sha256 | test("^[0-9a-f]{64}$"))))
      and (.cases | type == "array" and length > 0 and all(.[]; .status == "passed"))
      and ([.owners[].id] | sort) == ([$contract.owners[].id] | sort)
      and all(.owners[]; .status == "passed" and (.case_ids | length > 0) and (.refs | length > 0))
      and ([.commands[].id] | unique | length) == (.commands | length)
      and ([.cases[].source] | sort | unique) == ([$contract.owners[].cases[] | select(.kind == "runtime") | .source] | sort | unique)
      and all($contract.owners[]; . as $owner
        | any($observation.commands[]; .id == ("owner-" + ($owner.id | ltrimstr("std.")))))
    ' "$evidence" >/dev/null || die "execution evidence is stale or incomplete"

jq -e \
    --slurpfile contract "$contract" \
    '
      . as $observation
      | all($contract[0].owners[]; . as $owner
        | any($observation.owners[];
          .id == $owner.id and .rows == $owner.rows
          and .owner_command == $owner.owner_command
          and .refs == $owner.refs
          and (.case_ids | sort) == ($owner.cases | map(.id) | sort)))
    ' "$evidence" >/dev/null || die "observed owner/case declarations do not match the plan"

while IFS=$'\t' read -r log expected; do
    [[ -f "$log" ]] || die "missing command log: $log"
    [[ "$(sha256sum "$log" | cut -d' ' -f1)" == "$expected" ]] || die "command log changed: $log"
done < <(jq -r '.commands[] | [.log, .log_sha256] | @tsv' "$evidence")

for id in draft-adapter-build draft-validate draft-run; do
    jq -e --arg id "$id" 'any(.commands[]; .id == $id)' "$evidence" >/dev/null || die "missing command: $id"
done
while IFS= read -r command; do
    id="case-command-$(printf '%s' "$command" | sha256sum | cut -c1-12)"
    jq -e --arg id "$id" 'any(.commands[]; .id == $id)' "$evidence" >/dev/null || die "missing case command: $id"
done < <(jq -r '.owners[].cases[] | select(.kind != "runtime") | .command' "$contract" | sort -u)

suite_result="$(jq -r '.full_suite.result' "$evidence")"
[[ -f "$suite_result" ]] || die "missing full suite result: $suite_result"
[[ "$(sha256sum "$suite_result" | cut -d' ' -f1)" == "$(jq -r '.full_suite.result_sha256' "$evidence")" ]] || die "full suite result changed"
jq -e --slurpfile manifest conformance/0.1/manifest.json \
    --arg tree_sha256 "$tree_sha256" --arg manifest_sha256 "$manifest_sha256" '
    .format == "tondo-conformance-result-draft/2" and .passed == true
    and .tree_sha256 == $tree_sha256 and .manifest_sha256 == $manifest_sha256
    and ([.cases[].id] | sort) == ([$manifest[0].cases[].id] | sort)
    and all(.cases[]; .repetitions > 0
      and (.observation_sha256 | length) == .repetitions
      and all(.observation_sha256[]; test("^[0-9a-f]{64}$")))
    ' "$suite_result" >/dev/null || die "full suite identity or case observations are incomplete"

echo "stdlib conformance: current declared-case observations verified; public row coverage and promotion remain pending"
