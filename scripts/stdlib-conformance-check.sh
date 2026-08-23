#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CONFORMANCE_CONTRACT:-testing/stdlib-conformance.json}"
evidence="${TONDO_STDLIB_CONFORMANCE_EVIDENCE:-target/reliability/evidence/stdlib-conformance.json}"
matrix="${TONDO_STDLIB_MATRIX:-testing/stdlib-matrix.json}"

die() {
    echo "stdlib conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root/"}"
[[ -f "$evidence" ]] || die "missing execution evidence: ${evidence#"$root/"}"

manifest_sha256="$(sha256sum conformance/0.1/manifest.json | cut -d' ' -f1)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
revision="$(git rev-parse HEAD)"

jq -e \
    --arg manifest_sha256 "$manifest_sha256" \
    --arg contract_sha256 "$contract_sha256" \
    --arg revision "$revision" \
    --slurpfile contract "$contract" \
    --slurpfile matrix "$matrix" \
    '
      ($contract[0]) as $contract
      | .format == "tondo-stdlib-conformance-evidence/1"
      and .edition == "0.1"
      and .phase == "STD-0.1A"
      and .status == "passed"
      and .revision == $revision
      and .contract_sha256 == $contract_sha256
      and .manifest_sha256 == $manifest_sha256
      and .full_suite.passed == true
      and .full_suite.cases == 206
      and (.commands | type == "array" and length > 0 and all(.[]; .status == "passed" and (.log_sha256 | test("^[0-9a-f]{64}$"))))
      and (.cases | type == "array" and length > 0 and all(.[]; .status == "passed"))
      and ([.owners[].id] | sort) == ([$contract.owners[].id] | sort)
      and all(.owners[]; .status == "passed" and (.case_ids | length > 0) and (.refs | length > 0))
      and ([ $matrix[0].owners[].stages.CONF.status ] | unique) == ["verified"]
    ' "$evidence" >/dev/null || die "execution evidence is stale, incomplete, or matrix CONF is not promoted"

jq -e \
    --slurpfile contract "$contract" \
    --argjson required_rows "$(jq '[.owners[].rows.total] | add' "$contract")" \
    '
      ([.owners[].rows.total] | add) == $required_rows
      and ([.owners[].case_ids[]] | unique | length) > 0
    ' "$evidence" >/dev/null || die "owner row closure is incomplete"

while IFS= read -r ref; do
    base="${ref%%#*}"
    [[ -e "$root/$base" ]] || die "missing conformance reference: $ref"
done < <(jq -r '.owners[].refs[]' "$contract")

echo "stdlib conformance: OK (22 owners; 385 rows; current 206-case observation)"
