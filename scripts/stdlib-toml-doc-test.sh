#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-doc.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.toml documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

sed '/^### Data versus project manifest$/d' docs/contracts/stdlib-toml.md \
    > "$tmp_dir/missing-section.md"
expect_failure missing-section \
    env TONDO_STDLIB_TOML_DOCUMENT="$tmp_dir/missing-section.md" \
    scripts/stdlib-toml-doc-check.sh

for mutation in \
    '.documentation.status = "pending"' \
    '.documentation.examples = .documentation.examples[0:3]' \
    '.documentation.expected_stdout = "wrong"' \
    '.documentation.public_tondo_api = "verified"' \
    '.documentation.native_aot = "verified"' \
    '.promotion.next_blocks = ["STD-TOML-DOC-001"]'; do
    jq "$mutation" testing/stdlib-toml.json > "$tmp_dir/mutated.json"
    expect_failure "$mutation" \
        env TONDO_STDLIB_TOML_DOC_CONTRACT="$tmp_dir/mutated.json" \
        scripts/stdlib-toml-doc-check.sh
done

sed '/^fn errors_and_limits()/d' crates/tondo-stdlib/examples/toml_usage.rs \
    > "$tmp_dir/missing-example.rs"
expect_failure missing-example \
    env TONDO_STDLIB_TOML_DOC_EXAMPLE="$tmp_dir/missing-example.rs" \
    scripts/stdlib-toml-doc-check.sh

bash -n scripts/stdlib-toml-doc-check.sh scripts/stdlib-toml-doc-test.sh
echo "std.toml documentation tests: OK (sections, record, boundary and example negatives rejected)"
