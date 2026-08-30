#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_ASYNC_GROUP_DOCUMENT:-docs/contracts/stdlib-async-group.md}"
contract="${TONDO_STDLIB_ASYNC_GROUP_CONTRACT:-testing/stdlib-async-group.json}"
fixture="${TONDO_STDLIB_ASYNC_GROUP_FIXTURE:-tests/runtime/m11-std-async-group-001.to}"

die() {
    echo "std.async.Group documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: $document"
[[ -f "$contract" ]] || die "missing contract: $contract"
[[ -f "$fixture" ]] || die "missing executable fixture: $fixture"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.async.group"
  and .documentation.task == "STD-ASYNC-GROUP-DOC-001"
  and .documentation.status == "verified"
  and .documentation.document == "docs/contracts/stdlib-async-group.md"
  and .documentation.fixture == "tests/runtime/m11-std-async-group-001.to"
  and .documentation.command == "scripts/stdlib-async-group-doc-check.sh"
  and .documentation.expected_stdout == "group-ok"
  and .documentation.examples == [
    "fan-out-fan-in-all",
    "settle-mixed-outcomes",
    "next-completion-order",
    "select-commit-rollback",
    "cancel-drain"
  ]
  and .documentation.sections == [
    "surface",
    "ownership",
    "ordering",
    "errors",
    "cancellation",
    "costs",
    "executable-examples"
  ]
  and .implementation.required_follow_ups == []
  and .promotion.implementation_pending == []
' "$contract" >/dev/null || die "Group contract does not close its documentation record"

for marker in \
    '# Guía ejecutable de `std.async.Group`' \
    '## Cuándo usar `Group`' \
    '## Superficie pública' \
    '## Ownership y ciclo de vida' \
    '## Orden y outcomes' \
    '## `all`: éxito o error principal' \
    '## `settle`: conservar cada resultado' \
    '## `next`: finalización incremental y `select`' \
    '## `cancel`: terminar sin publicar un error' \
    '## Coste y límites' \
    '## Verificación ejecutable' \
    '## Diagnóstico privado' \
    'pub type Group[T, E]' \
    'pub type Completion[T, E] = {' \
    'pub fn group[T, E](): Group[T, E]' \
    'pub fn Group.add(var self, job: Join[T, E]): Unit' \
    'pub fn Group.all(self): Array[T] ! E suspends' \
    'pub fn Group.settle(self): Array[T ! E] suspends' \
    'pub fn Group.next(var self): Completion[T, E]? selectable' \
    'pub fn Group.cancel(self): Unit suspends' \
    'fan-out-fan-in-all' \
    'settle-mixed-outcomes' \
    'next-completion-order' \
    'select-commit-rollback' \
    'cancel-drain' \
    'STD-ASYNC-GROUP-DOC-001' \
    'm11-std-async-group-001.to' \
    'stdlib-async-group-performance.md' \
    'stdlib-async-group-conformance.md' \
    'scripts/stdlib-async-group-doc-check.sh'; do
    grep -Fq "$marker" "$document" || die "document misses marker: $marker"
done

if grep -Fq 'única leaf pendiente' "$document" ||
    grep -Fq 'requiere cerrar `DOC`' "$document"; then
    die "document contains a stale pending-DOC claim"
fi

fixture_root="${fixture%.to}"
[[ -f "$fixture_root.exit" ]] || die "fixture lacks exit sidecar: $fixture"
[[ -f "$fixture_root.stdout" ]] || die "fixture lacks stdout sidecar: $fixture"
[[ "$(tr -d '\r\n' < "$fixture_root.exit")" == "0" ]] || die "fixture exit sidecar is not zero"
[[ "$(tr -d '\r\n' < "$fixture_root.stdout")" == "group-ok" ]] || die "fixture stdout sidecar is not group-ok"

for function_name in \
    'all_cancels_pending' \
    'settle_mixed' \
    'next_observes_completion_order' \
    'next_select_commits_once' \
    'next_select_rolls_back' \
    'cancel_drains_pending' \
    'next_select_drains_cancelled_arm'; do
    grep -Fq "$function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

runtime_output="$(cargo run -q -p tondo-cli -- run "$fixture")" || die "executable fixture failed"
[[ "$runtime_output" == "group-ok" ]] || die "fixture output is not group-ok: $runtime_output"

echo "std.async.Group documentation: OK (normative guide, five executable example families, fixture group-ok)"
