#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.toml tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.wire.toml_version = "1.0.0"' testing/stdlib-toml.json > "$tmp_dir/toml-10.json"
expect_failure toml-10 env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/toml-10.json" scripts/stdlib-toml-check.sh

jq '.wire.documents = "multi-document"' testing/stdlib-toml.json > "$tmp_dir/multi-document.json"
expect_failure multi-document env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/multi-document.json" scripts/stdlib-toml-check.sh

jq '.wire.inline_tables = "mutable"' testing/stdlib-toml.json > "$tmp_dir/inline-extension.json"
expect_failure inline-extension env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/inline-extension.json" scripts/stdlib-toml-check.sh

jq '.duplicate_semantics.key = "last-wins"' testing/stdlib-toml.json > "$tmp_dir/duplicate-key.json"
expect_failure duplicate-key env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/duplicate-key.json" scripts/stdlib-toml-check.sh

jq '.date_time.more_than_nine_digits = "truncate"' testing/stdlib-toml.json > "$tmp_dir/date-precision.json"
expect_failure date-precision env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/date-precision.json" scripts/stdlib-toml-check.sh

jq '.dynamic_model.array = "homogeneous-only"' testing/stdlib-toml.json > "$tmp_dir/mixed-array.json"
expect_failure mixed-array env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/mixed-array.json" scripts/stdlib-toml-check.sh

jq '.capabilities.forbidden = [.capabilities.forbidden[] | select(. != "environment")]' testing/stdlib-toml.json > "$tmp_dir/ambient-environment.json"
expect_failure ambient-environment env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/ambient-environment.json" scripts/stdlib-toml-check.sh

jq '.streaming.stack = "host-recursive"' testing/stdlib-toml.json > "$tmp_dir/recursive-stack.json"
expect_failure recursive-stack env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/recursive-stack.json" scripts/stdlib-toml-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-toml.json > "$tmp_dir/premature-promotion.json"
expect_failure premature-promotion env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/premature-promotion.json" scripts/stdlib-toml-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-toml.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_TOML_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-toml-check.sh

for marker in \
    'TOML v1.1.0' \
    'TomlOffsetDateTime' \
    'TomlValueView' \
    'TomlEvent' \
    'multiline-basic' \
    'array-of-tables' \
    'DateTimePrecision' \
    'DuplicateTable' \
    'InlineTableExtension' \
    'TomlError.span' \
    'maxArrayTableRows' \
    'one-byte-chunks' \
    'tondo.toml' \
    'environment-interpolation' \
    'NoProgress'; do
    grep -Fq "$marker" docs/contracts/stdlib-toml.md \
        || { echo "std.toml tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-TOML-001"
  and .wire.toml_version == "1.1.0"
  and .wire.documents == "single-root-no-markers"
  and .wire.floats == ["fraction", "exponent", "inf", "nan"]
  and .wire.inline_tables == "closed-trailing-comma"
  and .dynamic_model.array == "ordered-heterogeneous-values"
  and .dynamic_model.shared_identity == false
  and .date_time.fraction_digits == "1..9"
  and .date_time.more_than_nine_digits == "reject"
  and .duplicate_semantics.key == "reject"
  and .duplicate_semantics.inline_table_extension == "reject"
  and .surface.selectable_operations == []
  and ([.surface.signatures[] | select(.id == "reader-from-reader" or .id == "reader-next" or .id == "writer-write") | .effect] | sort) == ["suspends", "suspends", "suspends"]
  and .streaming.chunk_boundary_invariant == true
  and .streaming.stack == "explicit-bounded-frames-and-worklists"
  and .ownership.reader_writer_affine == true
  and .errors.partial_success == false
  and .performance.scalar_oracle == true
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' testing/stdlib-toml.json >/dev/null

echo "std.toml tests: OK (TOML 1.1.0; dates; tables; duplicates; spans; streaming; toolchain boundary)"
