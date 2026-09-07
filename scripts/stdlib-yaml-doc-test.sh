#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-yaml-doc.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.yaml documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

sed '/^### Subset seguro y policies$/d' docs/contracts/stdlib-yaml.md \
    > "$tmp_dir/missing-section.md"
expect_failure missing-section \
    env TONDO_STDLIB_YAML_DOCUMENT="$tmp_dir/missing-section.md" \
    scripts/stdlib-yaml-doc-check.sh

jq '.documentation.status = "pending"' testing/stdlib-yaml.json \
    > "$tmp_dir/pending.json"
expect_failure pending-status \
    env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/pending.json" \
    scripts/stdlib-yaml-doc-check.sh

jq '.documentation.examples = .documentation.examples[0:5]' \
    testing/stdlib-yaml.json > "$tmp_dir/missing-example.json"
expect_failure missing-example \
    env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/missing-example.json" \
    scripts/stdlib-yaml-doc-check.sh

jq '.documentation.expected_stdout = "wrong-output"' \
    testing/stdlib-yaml.json > "$tmp_dir/wrong-output.json"
expect_failure wrong-output \
    env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/wrong-output.json" \
    scripts/stdlib-yaml-doc-check.sh

jq '.promotion.next_blocks = ["STD-YAML-DOC-001"]' \
    testing/stdlib-yaml.json > "$tmp_dir/stale-promotion.json"
expect_failure stale-promotion \
    env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/stale-promotion.json" \
    scripts/stdlib-yaml-doc-check.sh

jq '.documentation.sections = .documentation.sections[0:9]' \
    testing/stdlib-yaml.json > "$tmp_dir/missing-verification-section.json"
expect_failure missing-verification-section \
    env TONDO_STDLIB_YAML_CONTRACT="$tmp_dir/missing-verification-section.json" \
    scripts/stdlib-yaml-doc-check.sh

bash -n scripts/stdlib-yaml-doc-check.sh scripts/stdlib-yaml-doc-test.sh
echo "std.yaml documentation tests: OK (sections, status, examples, promotion and executable-boundary negatives rejected)"
