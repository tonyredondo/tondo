#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-target}"
evidence="$target_dir/reliability/evidence"
mkdir -p "$evidence"
output="${TONDO_DOC_TEST_OUTPUT:-$evidence/doc-test.json}"
mkdir -p "$(dirname "$output")"
temporary="${output}.tmp.$$.json"
trap 'rm -f "$temporary"' EXIT

cargo run -p tondo-cli --locked -- \
    doc-test --edition 0.1 TONDO_LANGUAGE_SPEC.md > "$temporary"

jq -e '
  type == "array"
  and all(.[];
    (keys_unsorted == ["file", "fence_byte", "category", "edition", "fixture", "fixture_sha256", "production", "source_sha256", "formatted_sha256", "parse_ok", "typecheck_ok", "expected_codes", "actual_codes"])
    and (.category as $category | (["syntax", "fragment", "script", "compile-fail", "pseudocode"] | index($category) != null))
    and (.edition == "0.1")
  )
  and ([.[].fence_byte] as $bytes | ($bytes == ($bytes | sort) and $bytes == ($bytes | unique)))
' "$temporary" >/dev/null

mv -f "$temporary" "$output"
trap - EXIT
echo "doc-test: OK ($(jq 'length' "$output") fences; atomic output: ${output#"$root"/})"
