#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-regex-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.regex tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.unicode.version = "15.1.0"' testing/stdlib-regex.json > "$tmp_dir/old-unicode.json"
expect_failure old-unicode env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/old-unicode.json" scripts/stdlib-regex-check.sh

jq '.engine.backtracking = "bounded"' testing/stdlib-regex.json > "$tmp_dir/backtracking.json"
expect_failure backtracking env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/backtracking.json" scripts/stdlib-regex-check.sh

jq '.syntax.unsupported = ["lookahead"]' testing/stdlib-regex.json > "$tmp_dir/open-syntax.json"
expect_failure open-syntax env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/open-syntax.json" scripts/stdlib-regex-check.sh

jq '.unicode.normalization = "NFC"' testing/stdlib-regex.json > "$tmp_dir/implicit-normalization.json"
expect_failure implicit-normalization env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/implicit-normalization.json" scripts/stdlib-regex-check.sh

jq '.surface.selectable_operations = ["find"]' testing/stdlib-regex.json > "$tmp_dir/selectable.json"
expect_failure selectable env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/selectable.json" scripts/stdlib-regex-check.sh

jq '(.surface.signatures[] | select(.id == "is-match") | .effect) = "suspends"' testing/stdlib-regex.json > "$tmp_dir/suspends.json"
expect_failure suspends env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/suspends.json" scripts/stdlib-regex-check.sh

jq '.semantics.zero_width_progress = "repeat-at-same-offset"' testing/stdlib-regex.json > "$tmp_dir/zero-progress.json"
expect_failure zero-progress env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/zero-progress.json" scripts/stdlib-regex-check.sh

jq '.replacement.callback = "function"' testing/stdlib-regex.json > "$tmp_dir/replacement-callback.json"
expect_failure replacement-callback env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/replacement-callback.json" scripts/stdlib-regex-check.sh

jq '.implementation.public_api_promoted = true' testing/stdlib-regex.json > "$tmp_dir/premature-promotion.json"
expect_failure premature-promotion env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/premature-promotion.json" scripts/stdlib-regex-check.sh

jq '.corpora += [.corpora[0]]' testing/stdlib-regex.json > "$tmp_dir/duplicate-corpus.json"
expect_failure duplicate-corpus env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/duplicate-corpus.json" scripts/stdlib-regex-check.sh

for marker in \
    'inicio más a la izquierda' \
    'longitud greedy más larga' \
    'longitud cero' \
    'Unicode 16.0.0' \
    'RegexOptions.ungreedy' \
    'InvalidUnicodeProperty' \
    'max_steps' \
    'max_output_bytes' \
    'InvalidReplacement' \
    'falta de memoria' \
    'callback'; do
    grep -Fq "$marker" docs/contracts/stdlib-regex.md \
        || { echo "std.regex tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-REGEX-001"
  and .unicode.version == "16.0.0"
  and .unicode.normalization == "none"
  and .syntax.grammar == "closed"
  and .syntax.lazy_quantifiers == true
  and .engine.backtracking == "forbidden"
  and .engine.host_recursion == "forbidden"
  and .semantics.find_all == "non-overlapping"
  and .semantics.zero_width_progress == "one-unicode-scalar"
  and .replacement.callback == "not-supported"
  and .surface.selectable_operations == []
  and ([.surface.signatures[] | select(.effect == "suspends")] | length) == 0
  and .ownership.iterator_outlives_input == false
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["STD-ID-001", "STD-LOG-001", "DIAG-RUNTIME-001"]
' testing/stdlib-regex.json >/dev/null

echo "std.regex tests: OK (syntax; Unicode; captures; zero-width progress; bounded linear safety)"
