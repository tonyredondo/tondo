#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_TEST_CONTRACT:-testing/stdlib-uuid-test.json}"
parent="${TONDO_STDLIB_UUID_CONTRACT:-testing/stdlib-uuid.json}"
corpus="${TONDO_STDLIB_UUID_TEST_CORPUS:-crates/tondo-reliability/tests/fixtures/uuid-cases.json}"
die() { echo "std.uuid tests: $*" >&2; exit 1; }
[[ -f "$contract" && -f "$corpus" && -f "$parent" ]] || die "missing testing inputs"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
TONDO_STDLIB_UUID_CONTRACT="$parent" bash scripts/stdlib-uuid-check.sh >/dev/null
jq -e -L scripts --slurpfile parent "$parent" '
  include "stdlib_uuid_testing";
  uuid_testing_boundary
  and .status == $parent[0].model.status
  and .quality_gate == $parent[0].model.quality_gate
  and .target == $parent[0].model.selected_route
  and $parent[0].host.status == "verified-production-hosted"
' "$contract" >/dev/null || die "invalid testing boundary or parent progression"
jq -e '
  def nonnegative: type == "number" and . >= 0 and floor == .;
  def hex: type == "string" and test("^([0-9a-f]{2})*$");
  def operation:
    if .operation == "parse" then (.text | type == "string")
    elif .operation == "bytes" then (.bytes_hex | hex)
    elif .operation == "v4" then (.entropy_hex | hex)
    elif .operation == "v5" then (.namespace | type == "string" and length == 36)
      and (.name_hex | hex) and (.name_limit | nonnegative)
    elif .operation == "v7" then (.milliseconds | type == "number" and floor == .)
      and (.entropy_hex | hex)
    else false end;
  .format == "tondo-stdlib-uuid-corpus/1"
  and .source == "https://www.rfc-editor.org/rfc/rfc9562.html"
  and .scope == "rfc-vectors-and-owner-regressions"
  and (.valid | length) == 17 and (.invalid | length) == 37
  and ([.valid[].id, .invalid[].id] | unique | length) == 54
  and all((.valid + .invalid)[]; (.id | type == "string" and test("^[a-z0-9-]+$")) and operation)
  and all(.valid[]; (.value | test("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"))
    and (.version | nonnegative and . < 16)
    and (.variant | IN("Ncs", "Rfc9562", "Microsoft", "Future")))
  and all(.invalid[]; (.error | IN("InvalidTextLength", "InvalidCharacter", "InvalidSeparator",
    "InvalidUrnPrefix", "InvalidBytesLength", "NameLimitExceeded", "TimestampOutOfRange", "ProviderMisconfigured"))
    and has("offset") and (.offset == null or (.offset | nonnegative)))
' "$corpus" >/dev/null || die "invalid persistent UUID corpus"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done < <(jq -r '.contract, .parent_contract, .model.sources[], .corpus.path, .test.source, .fuzz.source, .fuzz.corpus' "$contract")
for path in scripts/stdlib-uuid-test-check.sh scripts/stdlib-uuid-test-test.sh scripts/stdlib-uuid-fuzz.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
if grep -Eq 'tondo_stdlib|tondo_compiler|tondo_vm|sha1::|getrandom::' crates/tondo-reliability/src/uuid_model.rs; then
    die "reference imports production code"
else
    [[ "$?" == 1 ]] || die "reference could not be inspected"
fi
[[ "$(grep -c '^#\[test\]' crates/tondo-reliability/tests/uuid_models.rs)" == 11 ]] || die "incomplete integration tests"
[[ -s fuzz/corpus/stdlib_uuid/seed ]] || die "missing fuzz seed"
grep -Fq 'name = "stdlib_uuid"' fuzz/Cargo.toml || die "missing fuzz target"
grep -Fq 'stdlib-uuid-test.md' docs/contracts/stdlib-uuid.md || die "missing parent documentation link"
grep -Fq 'stdlib-uuid-test.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing normative register link"
echo "std.uuid tests: OK ($(jq -r .status "$contract"); independent oracle, kernel and sealed VM transcript)"
