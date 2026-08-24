#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-encoding-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.encoding tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.wire.base64.whitespace = "ignore"' testing/stdlib-encoding.json > "$tmp_dir/whitespace.json"
expect_failure whitespace env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/whitespace.json" scripts/stdlib-encoding-check.sh

jq '.wire.base64.non_zero_pad_bits = "accept"' testing/stdlib-encoding.json > "$tmp_dir/non-zero-pad-bits.json"
expect_failure non-zero-pad-bits env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/non-zero-pad-bits.json" scripts/stdlib-encoding-check.sh

jq '.policies.base64.decode = "permissive"' testing/stdlib-encoding.json > "$tmp_dir/permissive-base64.json"
expect_failure permissive-base64 env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/permissive-base64.json" scripts/stdlib-encoding-check.sh

jq '.policies.hex.any_case.output = "Either"' testing/stdlib-encoding.json > "$tmp_dir/ambiguous-hex-output.json"
expect_failure ambiguous-hex-output env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/ambiguous-hex-output.json" scripts/stdlib-encoding-check.sh

jq '.streaming.resource_limit_push = "partial-state-change"' testing/stdlib-encoding.json > "$tmp_dir/non-atomic-limit.json"
expect_failure non-atomic-limit env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/non-atomic-limit.json" scripts/stdlib-encoding-check.sh

jq '.surface.selectable_operations = ["base64-push"]' testing/stdlib-encoding.json > "$tmp_dir/selectable.json"
expect_failure selectable env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/selectable.json" scripts/stdlib-encoding-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-encoding.json > "$tmp_dir/premature-promotion.json"
expect_failure premature-promotion env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/premature-promotion.json" scripts/stdlib-encoding-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-encoding.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_ENCODING_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-encoding-check.sh

for marker in \
    'Base64Options.standard' \
    'Base64Options.urlSafeUnpadded' \
    'HexOptions.anyCase' \
    'entrada Base64' \
    'quantum' \
    'NonCanonical' \
    'NoProgress' \
    'EncodingError.offset' \
    'writeAll' \
    'SIMD'; do
    grep -Fq "$marker" docs/contracts/stdlib-encoding.md \
        || { echo "std.encoding tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-ENCODING-001"
  and .dependencies == ["std.bytes", "std.io"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .wire.base64.padding == ["Required", "Omitted"]
  and .wire.base64.whitespace == "reject"
  and .wire.base64.non_zero_pad_bits == "reject"
  and .wire.hex.odd_length == "reject"
  and .policies.no_permissive_decode == true
  and .streaming.chunk_boundary_invariant == true
  and .streaming.finish_required == true
  and .streaming.resource_limit_push == "atomic-no-state-change"
  and .surface.selectable_operations == []
  and .ownership.stream_handles_affine == true
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["STD-REGEX-001", "DIAG-RUNTIME-001"]
' testing/stdlib-encoding.json >/dev/null

echo "std.encoding tests: OK (policy negatives; canonicality; chunking; limits; lifecycle; promotion boundary)"
