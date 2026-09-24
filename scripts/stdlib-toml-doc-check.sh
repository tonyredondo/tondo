#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_TOML_DOCUMENT:-docs/contracts/stdlib-toml.md}"
contract="${TONDO_STDLIB_TOML_DOC_CONTRACT:-testing/stdlib-toml.json}"
example="${TONDO_STDLIB_TOML_DOC_EXAMPLE:-crates/tondo-stdlib/examples/toml_usage.rs}"

die() {
    echo "std.toml documentation: $*" >&2
    exit 1
}

for path in "$document" "$contract" "$example"; do
    [[ -f "$path" ]] || die "missing input: $path"
    tail -c 1 "$path" | cmp -s <(printf '\n') || die "input must end with LF: $path"
    ! grep -nE $'\r|[[:blank:]]$' "$path" >/dev/null || die "CR or trailing whitespace: $path"
done

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.toml"
  and .documentation == {
    task: "STD-TOML-DOC-001",
    status: "verified-rust-kernel-usage",
    document: "docs/contracts/stdlib-toml.md",
    example: "crates/tondo-stdlib/examples/toml_usage.rs",
    command: "scripts/stdlib-toml-doc-check.sh",
    expected_stdout: "toml-doc-ok",
    examples: ["materialized-and-typed", "borrowed-view-and-costs", "buffered-events-and-lifecycle", "errors-and-limits"],
    sections: ["data-versus-project-manifest", "policies-and-limits", "errors-and-ownership", "costs-and-performance", "executable-kernel-example", "promotion-boundary"],
    public_tondo_api: "not-implemented",
    native_aot: "not-claimed"
  }
  and .implementation.required_follow_ups == []
  and .conformance.public_compiler_api == "not-implemented"
  and .conformance.hosted_production_registration == "not-implemented"
  and .conformance.native_abi == "not-implemented"
  and .conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-CBOR-IMPL-001"]
' "$contract" >/dev/null || die "owner documentation record or promotion boundary differs"

for marker in \
    '## Executable usage guide for `std.toml`' \
    '### Data versus project manifest' \
    '### Policies and limits' \
    '### Errors and ownership' \
    '### Costs and performance' \
    '### Executable kernel example' \
    '### Promotion boundary' \
    'tondo.toml' \
    'TomlLimits::defaults()' \
    'TomlError' \
    'parse_view' \
    'clone_value()' \
    'TomlReader::from_chunks' \
    'TomlWriter::to_writer' \
    'stdlib-toml-performance.md' \
    'STD-CBOR-IMPL-001' \
    'toml-doc-ok'; do
    grep -Fq "$marker" "$document" || die "document misses: $marker"
done

for function_name in \
    materialized_and_typed \
    borrowed_view_and_costs \
    buffered_events_and_lifecycle \
    errors_and_limits; do
    grep -Fq "fn $function_name()" "$example" || die "example misses: $function_name"
done

grep -Fq 'TomlReader::from_chunks' "$example" || die "example misses reader path"
grep -Fq 'TomlWriter::to_writer' "$example" || die "example misses writer path"
grep -Fq 'toml::decode_static' "$example" || die "example misses typed path"
grep -Fq 'TomlErrorKind::ResourceLimit' "$example" || die "example misses limit rejection"
! grep -Fq 'El siguiente bloque es STD-TOML-DOC-001' "$document" || die "stale pending DOC claim"

output="$(cargo run -q -p tondo-stdlib --example toml_usage --locked)" || die "example failed"
[[ "$output" == "toml-doc-ok" ]] || die "example output differs: $output"

echo "std.toml documentation: OK (Rust kernel usage, policies, costs and promotion boundary)"
