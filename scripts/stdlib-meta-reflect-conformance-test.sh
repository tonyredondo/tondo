#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
[[ $# == 0 || ($# == 1 && "$1" == --plan) ]] || exit 2
work="$(mktemp -d "${TMPDIR:-/tmp}/tondo-public-conformance.XXXXXX")"
trap 'rm -rf -- "$work"' EXIT
checker=scripts/stdlib-meta-reflect-conformance-check.sh
"$checker" --plan
reject() {
    if "$@" >"$work/rejected.log" 2>&1; then
        echo "stdlib public conformance: invalid evidence was accepted" >&2
        exit 1
    fi
}
for mutation in \
    '.owners = []' \
    '.owners[0].signatures = []' \
    '.owners[1].requirements = []' \
    '.owners[0].requirements[0].observables += ["new contract"]' \
    '.owners[0].requirements[0].dimensions.public_boundary = []' \
    '.owners[1].callable_evidence = ["missing-test"]' \
    '.unknown = true'; do
    jq "$mutation" testing/stdlib-meta-reflect-conformance.json > "$work/plan.json"
    reject "$checker" --plan --contract "$work/plan.json"
done
if [[ "${1:-}" != --plan ]]; then
    result="${CARGO_TARGET_DIR:-target}/reliability/evidence/conformance-result.json"
    "$checker" --result "$result"
    for mutation in \
        '.passed = false' \
        '.tree_sha256 = ("0" * 64)' \
        '.case_layers |= map(select(.id != "meta"))' \
        '(.case_layers[] | select(.id == "meta")).cases[0].evidence = []' \
        '(.case_layers[] | select(.id == "meta")).cases[0].observation_sha256 = ("0" * 64)' \
        '(.case_layers[] | select(.id == "meta")).cases[0].evidence[0].source_sha256 = ("0" * 64)' \
        '(.case_layers[] | select(.id == "meta")).cases[0].evidence[0].observation_sha256 = ("0" * 64)'; do
        jq "$mutation" "$result" > "$work/result.json"
        reject "$checker" --result "$work/result.json"
    done
fi
echo "stdlib public conformance rejection tests: OK"
