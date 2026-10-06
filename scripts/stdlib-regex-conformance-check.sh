#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_REGEX_CONFORMANCE_CONTRACT:-testing/stdlib-regex-conformance.json}"
parent="${TONDO_STDLIB_REGEX_CONTRACT:-testing/stdlib-regex.json}"
python3 -B scripts/stdlib_regex_conformance.py check-contract --contract "$contract"
status="$(jq -r '.status' "$contract")"
jq -e -L scripts --arg status "$status" '
  include "stdlib_regex_progression";
  regex_kernel_progression and .conformance.status == $status
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-applicable-pure-core"
  and .implementation.native_aot_lowering == "not-claimed"
' "$parent" >/dev/null || { echo "std.regex conformance contract: parent progression differs" >&2; exit 1; }
for marker in 'STD-REGEX-CONF-001' 'test-only' '41' '33' 'source-level' 'table counter' 'STD-REGEX-DOC-001'; do
    grep -Fq "$marker" docs/contracts/stdlib-regex-conformance.md || { echo "std.regex conformance contract: missing boundary $marker" >&2; exit 1; }
done
for runner in scripts/stdlib-regex-conformance-check.sh scripts/stdlib-regex-conformance-test.sh scripts/stdlib-regex-conformance.sh; do
    [[ -x "$runner" ]] || { echo "std.regex conformance contract: non-executable $runner" >&2; exit 1; }
done
grep -Fq 'stdlib-regex-conformance.json' TONDO_STANDARD_LIBRARY_SPEC.md
grep -Fq 'stdlib-regex-conformance.md' docs/contracts/stdlib-regex.md
echo "std.regex conformance contract: OK ($status; exact private adapter boundary)"
