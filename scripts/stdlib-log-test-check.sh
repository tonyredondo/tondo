#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_LOG_TEST_CONTRACT:-testing/stdlib-log-test.json}"
parent="${TONDO_STDLIB_LOG_CONTRACT:-testing/stdlib-log.json}"
corpus="${TONDO_STDLIB_LOG_TEST_CORPUS:-crates/tondo-reliability/tests/fixtures/log-cases.json}"
die() { echo "std.log testing: $*" >&2; exit 1; }
[[ -f "$contract" && -f "$parent" && -f "$corpus" ]] || die 'missing testing inputs'
tail -c 1 "$contract" | cmp -s <(printf '\n') || die 'contract must end with LF'
env TONDO_STDLIB_LOG_CONTRACT="$parent" bash scripts/stdlib-log-check.sh >/dev/null
jq -e -L scripts --slurpfile parent "$parent" '
  include "stdlib_log_testing";
  log_testing_boundary
  and .status == $parent[0].model.status
  and .quality_gate == $parent[0].model.quality_gate
  and .target == $parent[0].model.selected_route
' "$contract" >/dev/null || die 'invalid testing boundary or parent progression'
jq -e -L scripts 'include "stdlib_log_corpus"; log_corpus' "$corpus" >/dev/null \
  || die 'invalid authored corpus'
while IFS= read -r path; do [[ -s "$path" ]] || die "missing input: $path"; done \
  < <(jq -r '.contract,.parent_contract,.model.sources[],.test.sources[],.test.kernel_comparison_adapter,.fuzz.source,.fuzz.corpus' "$contract")
unit_count=0
while IFS= read -r path; do
    if grep -Eq 'tondo_stdlib|tondo_compiler|tondo_vm|serde|std::(fs|net|time)|SystemTime|Instant' "$path"; then
        die "reference imports a production provider: $path"
    else [[ "$?" == 1 ]] || die "reference could not be inspected: $path"; fi
    unit_count=$((unit_count + $(grep -c '^ *#\[test\]' "$path" || [[ "$?" == 1 ]])))
done < <(jq -r '.model.sources[]' "$contract")
[[ "$unit_count" == 14 ]] || die 'incomplete independent unit tests'
integration_count=0
while IFS= read -r path; do
    integration_count=$((integration_count + $(grep -c '^#\[test\]' "$path")))
done < <(jq -r '.test.sources[]' "$contract")
[[ "$integration_count" == 9 ]] || die 'incomplete integration tests'
for path in scripts/stdlib-log-test-check.sh scripts/stdlib-log-test-test.sh scripts/stdlib-log-fuzz.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
grep -Fq 'name = "stdlib_log"' fuzz/Cargo.toml || die 'missing fuzz target'
grep -Fq 'stdlib-log-test.md' docs/contracts/stdlib-log.md || die 'missing parent link'
grep -Fq 'stdlib-log-test.json' TONDO_STANDARD_LIBRARY_SPEC.md || die 'missing spec link'
echo "std.log testing: OK ($(jq -r .status "$contract"); independent bounded model and hosted replay)"
