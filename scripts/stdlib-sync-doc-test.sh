#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-doc-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

sed '/^## Ordering y deadlocks$/d' docs/contracts/stdlib-sync.md \
    > "$tmp_dir/missing-section.md"
expect_failure missing-section \
    env TONDO_STDLIB_SYNC_DOCUMENT="$tmp_dir/missing-section.md" \
    scripts/stdlib-sync-doc-check.sh

jq '.documentation.status = "pending"' testing/stdlib-sync.json \
    > "$tmp_dir/pending.json"
expect_failure pending-status \
    env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-sync-doc-check.sh

jq '.documentation.examples = .documentation.examples[0:5]' \
    testing/stdlib-sync.json > "$tmp_dir/missing-example.json"
expect_failure missing-example \
    env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/missing-example.json" \
    scripts/stdlib-sync-doc-check.sh

jq '.documentation.expected_stdout = "wrong-output"' \
    testing/stdlib-sync.json > "$tmp_dir/wrong-output.json"
expect_failure wrong-output \
    env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/wrong-output.json" \
    scripts/stdlib-sync-doc-check.sh

jq '.promotion.next_blocks = ["STD-SYNC-DOC-001"]' \
    testing/stdlib-sync.json > "$tmp_dir/stale-promotion.json"
expect_failure stale-promotion \
    env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/stale-promotion.json" \
    scripts/stdlib-sync-doc-check.sh

scripts/stdlib-sync-doc-check.sh >/dev/null
echo "std.sync documentation tests: OK (sections, status, examples, promotion and executable oracle negatives rejected)"
