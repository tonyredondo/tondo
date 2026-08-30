#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-async-group-doc-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.async.Group documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

sed '/^## Superficie pública$/d' docs/contracts/stdlib-async-group.md \
    > "$tmp_dir/missing-section.md"
expect_failure missing-section \
    env TONDO_STDLIB_ASYNC_GROUP_DOCUMENT="$tmp_dir/missing-section.md" \
    scripts/stdlib-async-group-doc-check.sh

jq '.documentation.status = "pending"' testing/stdlib-async-group.json \
    > "$tmp_dir/pending.json"
expect_failure pending-status \
    env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-async-group-doc-check.sh

jq '.documentation.examples = .documentation.examples[0:4]' \
    testing/stdlib-async-group.json > "$tmp_dir/missing-example.json"
expect_failure missing-example \
    env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/missing-example.json" \
    scripts/stdlib-async-group-doc-check.sh

jq '.documentation.expected_stdout = "wrong-output"' \
    testing/stdlib-async-group.json > "$tmp_dir/wrong-output.json"
expect_failure wrong-output \
    env TONDO_STDLIB_ASYNC_GROUP_CONTRACT="$tmp_dir/wrong-output.json" \
    scripts/stdlib-async-group-doc-check.sh

scripts/stdlib-async-group-doc-check.sh >/dev/null
echo "std.async.Group documentation tests: OK (sections, status, examples and executable oracle negatives rejected)"
