#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-cbor-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.cbor tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.wire.rfc = "7049"' testing/stdlib-cbor.json > "$tmp_dir/old-rfc.json"
expect_failure old-rfc env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/old-rfc.json" scripts/stdlib-cbor-check.sh

jq '.wire.root = "concatenated-sequence"' testing/stdlib-cbor.json > "$tmp_dir/sequence.json"
expect_failure sequence env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/sequence.json" scripts/stdlib-cbor-check.sh

jq '.wire.indefinite_kinds = ["array", "map"]' testing/stdlib-cbor.json > "$tmp_dir/no-string-chunks.json"
expect_failure no-string-chunks env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/no-string-chunks.json" scripts/stdlib-cbor-check.sh

jq '.wire.break = "value"' testing/stdlib-cbor.json > "$tmp_dir/free-break.json"
expect_failure free-break env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/free-break.json" scripts/stdlib-cbor-check.sh

jq '.values.undefined_distinct_from_null = false' testing/stdlib-cbor.json > "$tmp_dir/null-undefined.json"
expect_failure null-undefined env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/null-undefined.json" scripts/stdlib-cbor-check.sh

jq '.policies.deterministic_indefinite = "accept"' testing/stdlib-cbor.json > "$tmp_dir/deterministic-indefinite.json"
expect_failure deterministic-indefinite env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/deterministic-indefinite.json" scripts/stdlib-cbor-check.sh

jq '.deterministic.map_order = "insertion-order"' testing/stdlib-cbor.json > "$tmp_dir/map-order.json"
expect_failure map-order env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/map-order.json" scripts/stdlib-cbor-check.sh

jq '.wire.ordinary_nan = "normalize"' testing/stdlib-cbor.json > "$tmp_dir/ordinary-nan.json"
expect_failure ordinary-nan env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/ordinary-nan.json" scripts/stdlib-cbor-check.sh

jq '.streaming.stack = "host-recursive"' testing/stdlib-cbor.json > "$tmp_dir/recursive-stack.json"
expect_failure recursive-stack env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/recursive-stack.json" scripts/stdlib-cbor-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-cbor.json > "$tmp_dir/premature-promotion.json"
expect_failure premature-promotion env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/premature-promotion.json" scripts/stdlib-cbor-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-cbor.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_CBOR_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-cbor-check.sh

for marker in \
    'RFC 8949' \
    'major types 0 a 7' \
    'CborFloat16' \
    'CborTag' \
    'Undefined' \
    'StartBytes' \
    'StartText' \
    'StartArray(none)' \
    'InvalidBreak' \
    'IndefiniteNotAllowed' \
    'DeterministicKeyCollision' \
    'un quiet-NaN binario-16' \
    'CborRaw' \
    'tondo.toml' \
    'NoProgress'; do
    grep -Fq "$marker" docs/contracts/stdlib-cbor.md \
        || { echo "std.cbor tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-CBOR-001"
  and .wire.major_types == ["unsigned-integer", "negative-integer", "byte-string", "text-string", "array", "map", "tag", "simple-or-float"]
  and .wire.length_forms == ["definite", "indefinite"]
  and .wire.indefinite_kinds == ["byte-string", "text-string", "array", "map"]
  and .wire.special_simple_values == ["false", "true", "null", "undefined"]
  and .values.undefined_distinct_from_null == true
  and .values.integer_kinds == ["UInt", "Negative"]
  and .policies.unknown_tags.default == "preserve"
  and .policies.indefinite_decode.default == "accept"
  and .policies.deterministic_indefinite == "reject"
  and .deterministic.map_order == "deterministic-key-encoding-bytes"
  and .deterministic.key_collision == "reject"
  and ([.surface.signatures[] | select(.id == "reader-from-reader" or .id == "reader-next" or .id == "writer-write") | .effect] | sort) == ["suspends", "suspends", "suspends"]
  and .streaming.chunk_boundary_invariant == true
  and .streaming.stack == "explicit-bounded-frames-and-worklists"
  and .errors.partial_success == false
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["STD-LOG-001", "DIAG-RUNTIME-001"]
' testing/stdlib-cbor.json >/dev/null

echo "std.cbor tests: OK (RFC 8949; tags; undefined; indefinite chunks; deterministic ordering; limits)"
