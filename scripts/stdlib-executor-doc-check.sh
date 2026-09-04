#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_EXECUTOR_DOCUMENT:-$root/docs/contracts/stdlib-executor.md}"
contract="${TONDO_STDLIB_EXECUTOR_CONTRACT:-$root/testing/stdlib-executor.json}"
fixture="${TONDO_STDLIB_EXECUTOR_DOC_FIXTURE:-$root/tests/runtime/m11-std-executor-doc-001.to}"

die() {
    echo "std.executor documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: ${document#"$root"/}"
[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
[[ -f "$fixture" ]] || die "missing executable fixture: ${fixture#"$root"/}"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.executor"
  and .documentation == {
    task: "STD-EXEC-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-executor.md",
    fixture: "tests/runtime/m11-std-executor-doc-001.to",
    command: "scripts/stdlib-executor-doc-check.sh",
    expected_stdout: "executor-doc-ok",
    examples: [
      "scoped-join",
      "bounded-backpressure",
      "actor-mailbox",
      "blocking-bridge",
      "cancel-and-drain"
    ],
    sections: [
      "scopes",
      "pools",
      "actors",
      "blocking",
      "cancellation",
      "shutdown",
      "costs",
      "composition-examples"
    ]
  }
  and .implementation.required_follow_ups == []
  and .implementation.observed.remaining == []
  and .conformance.remaining == []
  and .performance.remaining == []
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "owner contract does not close the documentation record"

for path in \
    docs/contracts/stdlib-executor-performance.md \
    docs/contracts/stdlib-executor-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-executor.json \
    testing/stdlib-executor-test.json \
    testing/stdlib-executor-performance.json \
    testing/stdlib-executor-conformance.json \
    scripts/stdlib-executor-check.sh \
    scripts/stdlib-executor-doc-check.sh \
    scripts/stdlib-executor-doc-test.sh; do
    [[ -f "$root/$path" ]] || die "missing linked evidence: $path"
done

for marker in \
    '# Contrato de `std.executor`' \
    '## Scopes y pools' \
    '## Superficie pública' \
    '## Capacidad, saturación y errores' \
    '## Actores y mailboxes' \
    '## Shutdown, cancelación y scopes' \
    '## Costes y límites' \
    '## Ejemplos ejecutables de composición' \
    'scoped-join' \
    'bounded-backpressure' \
    'actor-mailbox' \
    'blocking-bridge' \
    'cancel-and-drain' \
    'scoped_join' \
    'bounded_backpressure' \
    'actor_mailbox' \
    'blocking_bridge' \
    'cancel_and_drain' \
    'STD-EXEC-DOC-001' \
    'DIAG-RUNTIME-001' \
    'testing/stdlib-executor.json' \
    'stdlib-executor-performance.md' \
    'stdlib-executor-conformance.md' \
    'scripts/stdlib-executor-doc-check.sh' \
    'executor-doc-ok' \
    'native AOT' \
    'API pública'; do
    grep -Fq "$marker" "$document" || die "document misses marker: $marker"
done

if grep -Fq 'sigue pendiente `STD-EXEC-DOC-001`' "$document" ||
    grep -Fq 'siguiente bloque de promoción es `STD-EXEC-DOC-001`' "$document"; then
    die "document contains a stale pending-DOC claim"
fi

fixture_root="${fixture%.to}"
[[ -f "$fixture_root.exit" ]] || die "fixture lacks exit sidecar: ${fixture_root##*/}.exit"
[[ -f "$fixture_root.stdout" ]] || die "fixture lacks stdout sidecar: ${fixture_root##*/}.stdout"
[[ -f "$fixture_root.capabilities" ]] || die "fixture lacks capabilities sidecar: ${fixture_root##*/}.capabilities"
[[ "$(tr -d '\r\n' <"$fixture_root.exit")" == "0" ]] || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' <"$fixture_root.stdout")" == "executor-doc-ok" ]] || die "fixture stdout is not executor-doc-ok"
[[ "$(tr -d '\r\n' <"$fixture_root.capabilities")" == "threads" ]] \
    || die "fixture capabilities sidecar must declare threads"

for function_name in \
    'scoped_join' \
    'bounded_backpressure' \
    'actor_mailbox' \
    'blocking_bridge' \
    'cancel_and_drain'; do
    grep -Fq "$function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
project_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-executor-doc.XXXXXX")"
trap 'rm -rf -- "$project_dir"' EXIT
mkdir -p "$project_dir/src"
cp "$fixture" "$project_dir/src/main.to"
printf '%s\n' \
    '[package]' \
    'name = "executordoc"' \
    '' \
    '[target]' \
    'capabilities = ["console", "clock", "threads"]' \
    >"$project_dir/tondo.toml"

runtime_output="$(CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- run --project "$project_dir")" \
    || die "executable fixture failed"
[[ "$runtime_output" == "executor-doc-ok" ]] || die "fixture output is not executor-doc-ok: $runtime_output"

echo "std.executor documentation: OK (five executable composition families; scopes, pools, actors, blocking, cancellation, shutdown and costs explicit)"
