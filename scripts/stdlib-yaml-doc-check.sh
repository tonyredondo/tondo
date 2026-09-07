#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_YAML_DOCUMENT:-docs/contracts/stdlib-yaml.md}"
contract="${TONDO_STDLIB_YAML_CONTRACT:-testing/stdlib-yaml.json}"
fixture="${TONDO_STDLIB_YAML_DOC_FIXTURE:-tests/runtime/m11-std-yaml-doc-001.to}"

die() {
    echo "std.yaml documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: $document"
[[ -f "$contract" ]] || die "missing owner contract: $contract"
[[ -f "$fixture" ]] || die "missing executable fixture: $fixture"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.yaml"
  and .documentation == {
    task: "STD-YAML-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-yaml.md",
    fixture: "tests/runtime/m11-std-yaml-doc-001.to",
    command: "scripts/stdlib-yaml-doc-check.sh",
    expected_stdout: "yaml-doc-ok",
    examples: [
      "safe-subset-and-policies",
      "materialized-and-typed",
      "aliases-and-limits",
      "streaming-events",
      "errors-and-security",
      "costs-and-ownership"
    ],
    sections: [
      "surface",
      "safe-subset",
      "policies",
      "limits",
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
  and .promotion.next_blocks == ["STD-TOML-IMPL-001"]
' "$contract" >/dev/null || die "owner contract does not close the documentation record"

for marker in \
    '# Contrato de `std.yaml`' \
    '## Guía ejecutable de `std.yaml`' \
    '### Subset seguro y policies' \
    '### Límites y costes' \
    '### Errores y ownership' \
    '### Ejemplos materializados' \
    '### Ejemplos streaming' \
    '### Verificación ejecutable' \
    'safe-subset-and-policies' \
    'materialized-and-typed' \
    'aliases-and-limits' \
    'streaming-events' \
    'errors-and-security' \
    'costs-and-ownership' \
    'STD-YAML-DOC-001' \
    'STD-TOML-IMPL-001' \
    'tests/runtime/m11-std-yaml-doc-001.to' \
    'testing/stdlib-yaml.json' \
    'stdlib-yaml-performance.md' \
    'stdlib-yaml-conformance.md' \
    'scripts/stdlib-yaml-doc-check.sh' \
    'yaml-doc-ok' \
    'YAML 1.2 Core' \
    'YamlLimits.maxExpandedNodes' \
    'AliasCycle' \
    'YamlErrorKind.Closed' \
    'writer-boundary: static-contract-only-until-async-dispatch' \
    'unsupported VM host call' \
    'native_aot_lowering: not-claimed' \
    'simd: not-measured-no-optimized-route'; do
    grep -Fq "$marker" "$document" || die "document misses marker: $marker"
done

if grep -Fq 'La siguiente hoja es `STD-YAML-DOC-001`' "$document" ||
    grep -Fq 'La única hoja pendiente es la guía de uso `STD-YAML-DOC-001`' "$document" ||
    grep -Fq 'La documentación de uso queda pendiente de:' "$document"; then
    die "document contains a stale pending-DOC claim"
fi

fixture_root="${fixture%.to}"
[[ -f "$fixture_root.exit" ]] || die "fixture lacks exit sidecar: $fixture_root.exit"
[[ -f "$fixture_root.stdout" ]] || die "fixture lacks stdout sidecar: $fixture_root.stdout"
[[ "$(tr -d '\r\n' < "$fixture_root.exit")" == "0" ]] || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' < "$fixture_root.stdout")" == "yaml-doc-ok" ]] || die "fixture stdout sidecar is not yaml-doc-ok"

for function_name in \
    'safe_subset' \
    'materialized_typed' \
    'aliases_and_limits' \
    'streaming_events' \
    'errors_and_security' \
    'costs_and_ownership'; do
    grep -Fq "fn $function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

grep -Fq 'YamlReader.fromBytes' "$fixture" || die "fixture misses executable reader example"
grep -Fq 'yaml.parseAll' "$fixture" || die "fixture misses multi-document example"
grep -Fq 'yaml.parseView' "$fixture" || die "fixture misses borrowed-view example"
! grep -Fq 'YamlWriter.toWriter' "$fixture" || die "fixture cannot claim the unimplemented async writer route"

target_dir="${CARGO_TARGET_DIR:-target}"
runtime_output="$(CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- run "$fixture")" \
    || die "executable fixture failed"
[[ "$runtime_output" == "$(tr -d '\r\n' < "$fixture_root.stdout")" ]] \
    || die "fixture output differs from its stdout sidecar: $runtime_output"

echo "std.yaml documentation: OK (safe subset; policies; limits; costs; materialized/streaming examples; writer boundary explicit)"
