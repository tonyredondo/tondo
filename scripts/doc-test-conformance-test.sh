#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

records="${TONDO_DOC_TEST_OUTPUT:-${CARGO_TARGET_DIR:-target}/reliability/evidence/doc-test.json}"
[[ -f "$records" ]] || {
    echo "doc-test conformance tests: missing records from scripts/doc-test.sh" >&2
    exit 1
}

temporary="$(mktemp -d "${TMPDIR:-/tmp}/tondo-doc-test-negative.XXXXXX")"
trap 'rm -rf "$temporary"' EXIT

expect_failure() {
    local name="$1"
    local registry="$2"
    if TONDO_DOC_TEST_LINKS="$registry" scripts/doc-test-links-check.sh "$records" >/dev/null 2>&1; then
        echo "doc-test conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.links = .links[1:]' testing/doc-test-runtime-links.json > "$temporary/missing-link.json"
expect_failure missing-link "$temporary/missing-link.json"

jq '.links += [.links[0]]' testing/doc-test-runtime-links.json > "$temporary/duplicate-link.json"
expect_failure duplicate-link "$temporary/duplicate-link.json"

jq '(.links[] | select(.behavior == "runtime") | .evidence) = ["missing:test"]' \
    testing/doc-test-runtime-links.json > "$temporary/missing-evidence.json"
expect_failure missing-evidence "$temporary/missing-evidence.json"

jq '(.links[] | select(.behavior == "static-only") | .reason) = ""' \
    testing/doc-test-runtime-links.json > "$temporary/empty-reason.json"
expect_failure empty-reason "$temporary/empty-reason.json"

echo "doc-test conformance tests: OK"
