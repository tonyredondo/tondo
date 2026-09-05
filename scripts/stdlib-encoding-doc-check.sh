#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_ENCODING_DOCUMENT:-docs/contracts/stdlib-encoding.md}"
contract="${TONDO_STDLIB_ENCODING_CONTRACT:-testing/stdlib-encoding.json}"
fixture="${TONDO_STDLIB_ENCODING_DOC_FIXTURE:-tests/runtime/m11-std-encoding-doc-001.to}"

die() {
    echo "std.encoding documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: $document"
[[ -f "$contract" ]] || die "missing owner contract: $contract"
[[ -f "$fixture" ]] || die "missing executable fixture: $fixture"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.encoding"
  and .documentation == {
    task: "STD-ENCODING-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-encoding.md",
    fixture: "tests/runtime/m11-std-encoding-doc-001.to",
    command: "scripts/stdlib-encoding-doc-check.sh",
    expected_stdout: "Zm8=encoding-doc-ok",
    examples: [
      "policy-selection",
      "materialized-round-trip",
      "streaming-chunk-invariance",
      "strict-errors-and-limits",
      "costs-and-ownership",
      "writer-boundary"
    ],
    sections: [
      "surface",
      "policies",
      "errors",
      "costs",
      "ownership",
      "materialized-examples",
      "streaming-examples",
      "executable-verification"
    ]
  }
  and .implementation.required_follow_ups == []
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-YAML-IMPL-001"]
' "$contract" >/dev/null || die "owner contract does not close the documentation record"

for marker in \
    '# Contrato de `std.encoding`' \
    '## API fuente única' \
    '## Guía ejecutable de `std.encoding`' \
    '### Una forma por policy' \
    '### Errores observables' \
    '### Costes y límites' \
    '### Ejemplos materializados' \
    '### Ejemplos streaming' \
    '### Verificación ejecutable' \
    'Base64Options.standard' \
    'Base64Options.urlSafeUnpadded' \
    'HexOptions.anyCase' \
    'EncodingError.offset' \
    'ResourceLimit' \
    'EncodingErrorKind.Closed' \
    'bounded-carry' \
    'materialized_examples' \
    'streaming_examples' \
    'errors_and_limits' \
    'costs_and_ownership' \
    'writer_example' \
    'STD-ENCODING-DOC-001' \
    'STD-YAML-IMPL-001' \
    'tests/runtime/m11-std-encoding-doc-001.to' \
    'testing/stdlib-encoding.json' \
    'stdlib-encoding-performance.md' \
    'stdlib-encoding-conformance.md' \
    'scripts/stdlib-encoding-doc-check.sh' \
    'encoding-doc-ok' \
    'native_aot_lowering: not-claimed' \
    'simd: not-measured-no-optimized-route'; do
    grep -Fq "$marker" "$document" || die "document misses marker: $marker"
done

if grep -Fq 'Permanece pendiente únicamente `STD-ENCODING-DOC-001`' "$document" ||
    grep -Fq 'El siguiente bloque del owner es únicamente `STD-ENCODING-DOC-001`' "$document"; then
    die "document contains a stale pending-DOC claim"
fi

fixture_root="${fixture%.to}"
[[ -f "$fixture_root.exit" ]] || die "fixture lacks exit sidecar: $fixture_root.exit"
[[ -f "$fixture_root.stdout" ]] || die "fixture lacks stdout sidecar: $fixture_root.stdout"
[[ "$(tr -d '\r\n' < "$fixture_root.exit")" == "0" ]] || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' < "$fixture_root.stdout")" == "Zm8=encoding-doc-ok" ]] || die "fixture stdout sidecar is not the documentary marker"

for function_name in \
    'policy_selection' \
    'materialized_examples' \
    'streaming_examples' \
    'errors_and_limits' \
    'costs_and_ownership' \
    'writer_example'; do
    grep -Fq "fn $function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

target_dir="${CARGO_TARGET_DIR:-target}"
runtime_output="$(CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- run "$fixture")" \
    || die "executable fixture failed"
[[ "$runtime_output" == "$(tr -d '\r\n' < "$fixture_root.stdout")" ]] \
    || die "fixture output differs from its stdout sidecar: $runtime_output"

echo "std.encoding documentation: OK (one policy form; errors, costs, materialized/streaming examples and writer boundary)"
