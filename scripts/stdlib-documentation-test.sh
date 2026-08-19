#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-documentation-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib documentation: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners = .owners[1:]' testing/stdlib-documentation.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_DOCUMENTATION="$tmp_dir/missing-owner.json" \
    scripts/stdlib-documentation-check.sh

jq '.owners[0].examples = []' testing/stdlib-documentation.json \
    > "$tmp_dir/missing-example.json"
expect_failure missing-example env TONDO_STDLIB_DOCUMENTATION="$tmp_dir/missing-example.json" \
    scripts/stdlib-documentation-check.sh

jq '(.owners[] | select(.id == "std.serialization") | .boundary.public_api.status) = "complete"' \
    testing/stdlib-documentation.json > "$tmp_dir/overclaim-api.json"
expect_failure overclaim-api env TONDO_STDLIB_DOCUMENTATION="$tmp_dir/overclaim-api.json" \
    scripts/stdlib-documentation-check.sh

jq '(.owners[] | select(.id == "std.core") | .examples[0].source) = "tests/runtime/missing-example.to"' \
    testing/stdlib-documentation.json > "$tmp_dir/missing-sidecar.json"
expect_failure missing-sidecar env TONDO_STDLIB_DOCUMENTATION="$tmp_dir/missing-sidecar.json" \
    scripts/stdlib-documentation-check.sh

jq '(.owners[] | select(.id == "std.meta") | .runtime_reason) = null' \
    testing/stdlib-documentation.json > "$tmp_dir/missing-runtime-reason.json"
expect_failure missing-runtime-reason env TONDO_STDLIB_DOCUMENTATION="$tmp_dir/missing-runtime-reason.json" \
    scripts/stdlib-documentation-check.sh

jq -e '
  . as $root
  | $root.summary == {
    owners: 22,
    examples: 32,
    runtime_examples: 26,
    external_examples: 4,
    compiler_examples: 2,
    api_complete: 18,
    api_partial: 1,
    api_not_applicable: 3
  }
  and any($root.owners[]; .id == "std.meta" and .runtime_applicable == false and (.runtime_reason | length) > 0)
  and any($root.owners[]; .id == "std.reflect" and .runtime_applicable == false and (.runtime_reason | length) > 0)
  and all(["std.json", "std.messagepack", "std.protobuf"][];
    . as $owner_id
    | any($root.owners[]; .id == $owner_id and .boundary.public_api.status == "complete")
  )
  and any($root.owners[]; .id == "std.serialization" and .boundary.public_api.status == "partial")
' testing/stdlib-documentation.json >/dev/null

echo "stdlib documentation tests: OK"
