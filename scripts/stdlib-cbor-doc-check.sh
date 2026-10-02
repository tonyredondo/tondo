#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_CONTRACT:-testing/stdlib-cbor.json}"
document="${TONDO_STDLIB_CBOR_DOCUMENT:-docs/contracts/stdlib-cbor.md}"
# Always inspect and execute the same canonical example. There is no alternate
# source override whose contents could differ from the program Cargo runs.
example="crates/tondo-stdlib/examples/cbor_usage.rs"

die() {
    echo "std.cbor documentation: $*" >&2
    exit 1
}
for path in "$contract" "$document" "$example"; do
    [[ -f "$path" ]] || die "missing input: $path"
    tail -c 1 "$path" | cmp -s <(printf '\n') || die "input must end with LF: $path"
    ! grep -nE $'\r|[[:blank:]]$' "$path" >/dev/null || die "CR or trailing whitespace: $path"
done
jq -e -L scripts '
  include "stdlib_cbor_progression";
  .format == "tondo-stdlib-owner-contract/1" and .owner == "std.cbor"
  and cbor_owner_progression and cbor_documentation_metadata
  and .conformance.status == "verified-hosted-vm-adapter-and-native-stdlib-process"
  and .implementation.public_api_promoted == false
  and .implementation.host == "not-claimed-until-compiler-cbor-abi"
  and .implementation.native_aot_lowering == "not-claimed"
' "$contract" >/dev/null || die "documentation record or promotion boundary differs"

for marker in \
    '## Executable usage guide for `std.cbor`' \
    '### Values and byte preservation' \
    '### Tags and deterministic encoding' \
    '### Policies and limits' \
    '### Errors, ownership and terminal streams' \
    '### Costs and performance' \
    '### Executable kernel example' \
    '### Promotion boundary' \
    'CborLimits::default()' \
    'CborError' \
    'parse_view' \
    'view.own()' \
    'CborReader::from_reader' \
    'CborWriter::to_writer' \
    'External I/O is not transactional' \
    'including after terminal `finish()`' \
    'stdlib-cbor-performance.md' \
    'STD-REGEX-IMPL-001' \
    'cbor-doc-ok'; do
    grep -Fq "$marker" "$document" || die "document misses: $marker"
done
for function_name in materialized_and_typed tags_and_determinism raw_view_and_costs \
    buffered_events_and_lifecycle errors_and_limits partial_io_is_not_transactional; do
    grep -Fq "fn $function_name()" "$example" || die "example misses: $function_name"
done

output="$(cargo run -q -p tondo-stdlib --example cbor_usage --locked)" || die "canonical example failed"
[[ "$output" == "cbor-doc-ok" ]] || die "example output differs: $output"
echo "std.cbor documentation: OK (canonical Rust example, policies, costs and bounded promotion)"
