#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-text-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.text owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq 'del(.owners[] | select(. == "std.text"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-owner.json" \
    scripts/stdlib-core-check.sh

jq 'del(.test_matrix[] | select(. == "composition"))' testing/stdlib-core.json \
    > "$tmp_dir/missing-composition.json"
expect_failure missing-composition env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-composition.json" \
    scripts/stdlib-core-check.sh

jq '.owner = "std.text"' testing/stdlib-core.json > "$tmp_dir/wrong-owner.json"
expect_failure wrong-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/wrong-owner.json" \
    scripts/stdlib-core-check.sh

for signature in \
    'pub fn String.empty(): String' \
    'pub fn String.fromChars(chars: Array[Char]): String ! TextError' \
    'pub fn String.length(self): Int' \
    'pub fn String.byteLength(self): Int' \
    'pub fn String.get(self, index: Int): Char?' \
    'pub fn String.slice(self, start: Int, end: Int): String ! TextError' \
    'pub fn String.contains(self, needle: String): Bool' \
    'pub fn String.startsWith(self, prefix: String): Bool' \
    'pub fn String.endsWith(self, suffix: String): Bool' \
    'pub fn String.find(self, needle: String): Int?' \
    'pub fn String.replace(self, old: String, new: String): String' \
    'pub fn String.trim(self): String' \
    'pub fn String.toLowerAscii(self): String' \
    'pub fn String.toUpperAscii(self): String' \
    'pub fn String.chars(self): String'; do
    grep -Fq "$signature" docs/contracts/stdlib-core.md
done

for symbol in \
    'HirBootstrapHostFunction::TextFromChars' \
    'HirBootstrapHostFunction::TextLength' \
    'HirBootstrapHostFunction::TextByteLength' \
    'HirBootstrapHostFunction::TextGet' \
    'HirBootstrapHostFunction::TextSlice' \
    'HirBootstrapHostFunction::TextChars' \
    'HirBootstrapHostFunction::TextContains' \
    'HirBootstrapHostFunction::TextStartsWith' \
    'HirBootstrapHostFunction::TextEndsWith' \
    'HirBootstrapHostFunction::TextFind' \
    'HirBootstrapHostFunction::TextReplace' \
    'HirBootstrapHostFunction::TextTrim' \
    'HirBootstrapHostFunction::TextLowerAscii' \
    'HirBootstrapHostFunction::TextUpperAscii'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/hir/lower.rs
done

grep -Fq 'String(invalid)' tests/runtime/m11-std-text-002.to
grep -Fq 'rebuilt.chars()' tests/runtime/m11-std-text-002.to
grep -Fq 'rebuilt.slice(1, 3)?' tests/runtime/m11-std-text-002.to
grep -Fq 'String.fromChars' tests/runtime/m11-std-text-002.to
grep -Fq 'invalid-continuation' testing/stdlib-bytes.json
grep -Fq 'truncated-sequence' testing/stdlib-bytes.json

jq -e '
  ([.rows[] | select(.owner == "std.text")] | length) == 15
  and all(.rows[] | select(.owner == "std.text"); .missing == [])
' testing/stdlib-public-api.json >/dev/null

echo "std.text owner tests: OK"
