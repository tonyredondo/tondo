#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-uuid-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.uuid tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.standard.width_bits = 64' testing/stdlib-uuid.json > "$tmp_dir/wrong-width.json"
expect_failure wrong-width env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/wrong-width.json" scripts/stdlib-uuid-check.sh

jq '.versions.generated = ["v1", "v4", "v7"]' testing/stdlib-uuid.json > "$tmp_dir/unsupported-generation.json"
expect_failure unsupported-generation env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/unsupported-generation.json" scripts/stdlib-uuid-check.sh

jq '.versions.v7.capabilities = ["entropy"]' testing/stdlib-uuid.json > "$tmp_dir/missing-clock.json"
expect_failure missing-clock env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/missing-clock.json" scripts/stdlib-uuid-check.sh

jq '.versions.v7.strict_monotonicity = true' testing/stdlib-uuid.json > "$tmp_dir/strict-monotonic.json"
expect_failure strict-monotonic env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/strict-monotonic.json" scripts/stdlib-uuid-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "collision-registry")]' testing/stdlib-uuid.json > "$tmp_dir/collision-registry.json"
expect_failure collision-registry env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/collision-registry.json" scripts/stdlib-uuid-check.sh

jq '.text.compact_32_hex = "accept"' testing/stdlib-uuid.json > "$tmp_dir/compact-text.json"
expect_failure compact-text env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/compact-text.json" scripts/stdlib-uuid-check.sh

jq '.surface.signatures[] |= if .id == "v7" then .effect = "suspends" else . end' testing/stdlib-uuid.json > "$tmp_dir/suspends.json"
expect_failure suspends env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/suspends.json" scripts/stdlib-uuid-check.sh

jq '.ownership.global_registry = true' testing/stdlib-uuid.json > "$tmp_dir/global-registry.json"
expect_failure global-registry env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/global-registry.json" scripts/stdlib-uuid-check.sh

jq '.promotion.next_blocks = ["STD-ID-001", "STD-LOG-001"]' testing/stdlib-uuid.json > "$tmp_dir/premature-next.json"
expect_failure premature-next env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/premature-next.json" scripts/stdlib-uuid-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-uuid.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-uuid-check.sh

for marker in \
    '128 bits' \
    'network byte order' \
    'Uuid.parse' \
    'Uuid.toString()' \
    'Uuid.v4' \
    'Uuid.v5' \
    'Uuid.v7' \
    '122 bits' \
    '74 bits' \
    'No se reintenta silenciosamente' \
    'TimestampOutOfRange' \
    'InvalidTextLength' \
    'collision' \
    'monotonicidad estricta'; do
    grep -Fq "$marker" docs/contracts/stdlib-uuid.md \
        || { echo "std.uuid tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-ID-001"
  and .standard.width_bits == 128
  and .standard.byte_order == "network-big-endian"
  and .versions.generated == ["v4", "v5", "v7"]
  and .versions.v5.deterministic == true
  and .versions.v7.strict_monotonicity == false
  and .text.canonical == "8-4-4-4-12-lowercase-hex"
  and .text.compact_32_hex == "reject"
  and .capabilities.source_sets.v7 == ["civil-clock", "entropy"]
  and .surface.selectable_operations == []
  and ([.surface.signatures[] | select(.effect == "suspends")] | length) == 0
  and .ownership.global_registry == false
  and .performance.generation_state == "no-global-lock-counter-or-registry"
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' testing/stdlib-uuid.json >/dev/null

echo "std.uuid tests: OK (RFC vectors; text/bytes; v4/v5/v7; capabilities; no hidden state)"
