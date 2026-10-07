#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_TEST_CONTRACT:-testing/stdlib-net-test.json}"
parent="${TONDO_STDLIB_NET_CONTRACT:-testing/stdlib-net.json}"
corpus="${TONDO_STDLIB_NET_TEST_CORPUS:-crates/tondo-reliability/tests/fixtures/net-cases.json}"
die() { echo "std.net tests: $*" >&2; exit 1; }
[[ -f "$contract" && -f "$parent" && -f "$corpus" ]] || die 'missing testing inputs'
tail -c 1 "$contract" | cmp -s <(printf '\n') || die 'contract must end with LF'
TONDO_STDLIB_NET_CONTRACT="$parent" bash scripts/stdlib-net-check.sh >/dev/null
jq -e -L scripts --slurpfile parent "$parent" '
  include "stdlib_net_testing";
  net_testing_boundary
  and .status == $parent[0].model.status
  and .quality_gate == $parent[0].model.quality_gate
  and .target == $parent[0].model.selected_route
' "$contract" >/dev/null || die 'invalid testing boundary or parent progression'
jq -e '
  def uint: type == "number" and . >= 0 and floor == .;
  def integer: type == "number" and floor == .;
  def hex: type == "string" and test("^([0-9a-f]{2})*$") and length <= 256;
  def operation:
    if .operation == "hostname" then (.text | type == "string")
    elif .operation == "limits" then all([.read,.datagram,.results][]; integer)
    elif .operation == "read" then (.bytes_hex | hex) and (.requested | uint) and (.eof | type == "boolean")
    elif (.operation == "write" or .operation == "udp-send") then all([.requested,.reported][]; uint)
    elif .operation == "datagram" then (.bytes_hex | hex) and (.wire_length | uint)
      and (.port | uint and . <= 65535) and (.limit | uint and . > 0 and . <= 65535)
    elif .operation == "deadline" then all([.domain,.active][]; uint)
      and (.tick == null or (.tick | integer)) and (.now | integer) and (.cancelled | type == "boolean")
    elif .operation == "dns" then (.limit | uint and . > 0 and . <= 1024)
      and (.addresses | type == "array" and length <= 32)
      and all(.addresses[]; type == "array" and length == 2
        and (.[0] | uint and . <= 255) and (.[1] | uint and . <= 65535))
    else false end;
  .format == "tondo-stdlib-net-corpus/1" and .scope == "bounded-admission-and-owner-regressions"
  and (.cases | length) == 40 and ([.cases[].id] | unique | length) == 40
  and all(.cases[]; (.id | type == "string" and test("^[a-z0-9-]+$")) and operation
    and (has("value") != has("error"))
    and (if has("error") then (.error | IN("InvalidHostName","InvalidLimit","InvalidDeadline",
      "ResourceLimit","DatagramTooLarge","Cancelled","Timeout","Host","InvalidPort")) else true end))
' "$corpus" >/dev/null || die 'invalid persistent corpus'
while IFS= read -r path; do [[ -f "$path" ]] || die "missing input: $path"; done \
  < <(jq -r '.contract,.parent_contract,.model.sources[],.test.sources[],.fuzz.source,.fuzz.corpus' "$contract")
while IFS= read -r path; do
    if grep -Eq 'tondo_stdlib|tondo_compiler|tondo_vm|tokio::|rustls::|hickory|std::net|std::time' "$path"; then
        die "reference imports a production provider: $path"
    else [[ "$?" == 1 ]] || die "reference could not be inspected: $path"; fi
done < <(jq -r '.model.sources[]' "$contract")
unit_count=0
while IFS= read -r path; do
    unit_count=$((unit_count + $(grep -c '^ *#\[test\]' "$path")))
done < <(jq -r '.model.sources[]' "$contract")
[[ "$unit_count" == 14 ]] || die 'incomplete independent model tests'
integration_count=0
while IFS= read -r path; do
    integration_count=$((integration_count + $(grep -c '^#\[test\]' "$path")))
done < <(jq -r '.test.sources[]' "$contract")
[[ "$integration_count" == 9 ]] || die 'incomplete integration tests'
for path in scripts/stdlib-net-test-check.sh scripts/stdlib-net-test-test.sh scripts/stdlib-net-fuzz.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
grep -Fq 'name = "stdlib_net"' fuzz/Cargo.toml || die 'missing fuzz target'
[[ -s fuzz/corpus/stdlib_net/seed ]] || die 'missing fuzz seed'
grep -Fq 'stdlib-net-test.md' docs/contracts/stdlib-net.md || die 'missing parent link'
grep -Fq 'stdlib-net-test.json' TONDO_STANDARD_LIBRARY_SPEC.md || die 'missing spec link'
echo "std.net tests: OK ($(jq -r .status "$contract"); finite model and controlled public hosted replay)"
