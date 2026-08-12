#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

records="${1:-${TONDO_DOC_TEST_OUTPUT:-${CARGO_TARGET_DIR:-target}/reliability/evidence/doc-test.json}}"
links="${TONDO_DOC_TEST_LINKS:-testing/doc-test-runtime-links.json}"
inventory="${TONDO_TEST_INVENTORY:-testing/inventory.json}"

for input in "$records" "$links" "$inventory"; do
    [[ -f "$input" ]] || {
        echo "doc-test links: missing input: $input" >&2
        exit 1
    }
done

jq -n -e \
    --slurpfile records "$records" \
    --slurpfile links "$links" \
    --slurpfile inventory "$inventory" '
  ($records[0]) as $records
  | ($links[0]) as $links
  | ($inventory[0]) as $inventory
  | ($records | map(select(.category == "fragment" or .category == "script"))) as $typed
  | ($typed | map([.file, .fence_byte, .source_sha256])) as $typed_keys
  | ($links.links | map([.document, .fence_byte, .source_sha256])) as $link_keys
  | ($inventory.tests | map({key:.id, value:.}) | from_entries) as $tests
  | $links.format == "tondo-doc-test-runtime-links/1"
    and $links.edition == "0.1"
    and $links.rules == {
      typed_fences_are_classified: true,
      syntax_fences_make_no_runtime_claim: true,
      runtime_evidence_is_public_and_executable: true,
      documentation_runner_never_executes_examples: true
    }
    and (($links.documents | length) == ($links.documents | unique | length))
    and (($links.documents | sort) == ($records | map(.file) | unique | sort))
    and (($typed_keys | sort) == ($link_keys | sort))
    and (($link_keys | length) == ($link_keys | unique | length))
    and all($links.links[];
      (.document | type == "string")
      and (.fence_byte | type == "number")
      and (.source_sha256 | test("^[0-9a-f]{64}$"))
      and if .behavior == "runtime" then
        (.evidence | type == "array" and length > 0)
        and all(.evidence[];
          . as $id
          | $tests[$id].status == "executable"
          and ($tests[$id].kind == "conformance-case" or $tests[$id].kind == "rust-test")
        )
        and (has("reason") | not)
      elif .behavior == "static-only" then
        (.evidence == []) and (.reason | type == "string" and length > 0)
      else false
      end
    )
' >/dev/null || {
    echo "doc-test links: typed fences or public runtime evidence do not match" >&2
    exit 1
}

runtime="$(jq '[.links[] | select(.behavior == "runtime")] | length' "$links")"
static="$(jq '[.links[] | select(.behavior == "static-only")] | length' "$links")"
echo "doc-test links: OK ($runtime runtime; $static static-only)"
