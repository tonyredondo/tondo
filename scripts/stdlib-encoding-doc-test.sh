#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-encoding-doc.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.encoding documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

sed '/^### Ejemplos materializados$/d' docs/contracts/stdlib-encoding.md \
    > "$tmp_dir/missing-section.md"
expect_failure missing-section \
    env TONDO_STDLIB_ENCODING_DOCUMENT="$tmp_dir/missing-section.md" \
    scripts/stdlib-encoding-doc-check.sh

jq '.documentation.status = "pending"' testing/stdlib-encoding.json \
    > "$tmp_dir/pending.json"
expect_failure pending-status \
    env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-encoding-doc-check.sh

jq '.documentation.examples = .documentation.examples[0:5]' \
    testing/stdlib-encoding.json > "$tmp_dir/missing-example.json"
expect_failure missing-example \
    env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/missing-example.json" \
    scripts/stdlib-encoding-doc-check.sh

jq '.documentation.expected_stdout = "wrong-output"' \
    testing/stdlib-encoding.json > "$tmp_dir/wrong-output.json"
expect_failure wrong-output \
    env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/wrong-output.json" \
    scripts/stdlib-encoding-doc-check.sh

jq '.promotion.next_blocks = ["STD-ENCODING-DOC-001"]' \
    testing/stdlib-encoding.json > "$tmp_dir/stale-promotion.json"
expect_failure stale-promotion \
    env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/stale-promotion.json" \
    scripts/stdlib-encoding-doc-check.sh

jq '.documentation.sections = .documentation.sections[0:7]' \
    testing/stdlib-encoding.json > "$tmp_dir/missing-ownership-section.json"
expect_failure missing-ownership-section \
    env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/missing-ownership-section.json" \
    scripts/stdlib-encoding-doc-check.sh

bash -n scripts/stdlib-encoding-doc-check.sh scripts/stdlib-encoding-doc-test.sh
scripts/stdlib-encoding-doc-check.sh >/dev/null
echo "std.encoding documentation tests: OK (sections, status, examples, promotion and fixture negatives rejected)"
