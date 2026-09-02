#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_CHANNEL_DOCUMENT:-docs/contracts/stdlib-channel.md}"
contract="${TONDO_STDLIB_CHANNEL_CONTRACT:-testing/stdlib-channel.json}"
fixture="${TONDO_STDLIB_CHANNEL_DOC_FIXTURE:-tests/runtime/m11-std-channel-doc-001.to}"

die() {
    echo "std.channel documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: $document"
[[ -f "$contract" ]] || die "missing owner contract: $contract"
[[ -f "$fixture" ]] || die "missing executable fixture: $fixture"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.channel"
  and .documentation == {
    task: "STD-CHANNEL-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-channel.md",
    fixture: "tests/runtime/m11-std-channel-doc-001.to",
    command: "scripts/stdlib-channel-doc-check.sh",
    expected_stdout: "channel-doc-ok",
    examples: [
      "fan-out-fan-in",
      "pipeline-backpressure",
      "select-cancel-safe",
      "close-and-drain",
      "discardable-iteration"
    ],
    sections: [
      "surface",
      "ordering",
      "closure",
      "cancellation",
      "fairness",
      "costs",
      "composition-examples"
    ]
  }
  and .implementation.required_follow_ups == []
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-EXEC-IMPL-001"]
' "$contract" >/dev/null || die "owner contract does not close the documentation record"

for marker in \
    '# Contrato de `std.channel`' \
    '## Superficie pública' \
    '## Capacidad y orden' \
    '## Cierre y estados' \
    '## `select`, cancelación y fairness' \
    '## Costes y límites' \
    '## Ejemplos ejecutables de composición' \
    'fan-out-fan-in' \
    'pipeline-backpressure' \
    'select-cancel-safe' \
    'close-and-drain' \
    'discardable-iteration' \
    'fan_out_fan_in' \
    'pipeline_backpressure' \
    'select_cancel_safe' \
    'close_and_drain' \
    'discardable_iteration' \
    'STD-CHANNEL-DOC-001' \
    'STD-EXEC-IMPL-001' \
    'tests/runtime/m11-std-channel-doc-001.to' \
    'testing/stdlib-channel.json' \
    'stdlib-channel-performance.md' \
    'stdlib-channel-conformance.md' \
    'scripts/stdlib-channel-doc-check.sh' \
    'channel-doc-ok'; do
    grep -Fq "$marker" "$document" || die "document misses marker: $marker"
done

if grep -Fq 'La implementación pública permanece pendiente' "$document" ||
    grep -Fq 'siguiente bloque de promoción es `STD-CHANNEL-DOC-001`' "$document"; then
    die "document contains a stale pending-DOC claim"
fi

fixture_root="${fixture%.to}"
[[ -f "$fixture_root.exit" ]] || die "fixture lacks exit sidecar: $fixture"
[[ -f "$fixture_root.stdout" ]] || die "fixture lacks stdout sidecar: $fixture"
[[ "$(tr -d '\r\n' < "$fixture_root.exit")" == "0" ]] || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' < "$fixture_root.stdout")" == "channel-doc-ok" ]] || die "fixture stdout is not channel-doc-ok"

for function_name in \
    'fan_out_fan_in' \
    'pipeline_backpressure' \
    'select_cancel_safe' \
    'close_and_drain' \
    'discardable_iteration'; do
    grep -Fq "$function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

runtime_output="$(cargo run -q -p tondo-cli --locked -- run "$fixture")" || die "executable fixture failed"
[[ "$runtime_output" == "channel-doc-ok" ]] || die "fixture output is not channel-doc-ok: $runtime_output"

echo "std.channel documentation: OK (five executable composition families; ordering, closure, cancellation, fairness and costs explicit)"
