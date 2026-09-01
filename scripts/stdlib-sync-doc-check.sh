#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

document="${TONDO_STDLIB_SYNC_DOCUMENT:-docs/contracts/stdlib-sync.md}"
contract="${TONDO_STDLIB_SYNC_CONTRACT:-testing/stdlib-sync.json}"
fixture="${TONDO_STDLIB_SYNC_DOC_FIXTURE:-tests/runtime/m11-std-sync-doc-001.to}"

die() {
    echo "std.sync documentation: $*" >&2
    exit 1
}

[[ -f "$document" ]] || die "missing document: $document"
[[ -f "$contract" ]] || die "missing owner contract: $contract"
[[ -f "$fixture" ]] || die "missing executable fixture: $fixture"
tail -c 1 "$document" | cmp -s <(printf '\n') || die "document must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$document" >/dev/null || die "document contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.sync"
  and .documentation.task == "STD-SYNC-DOC-001"
  and .documentation.status == "verified"
  and .documentation.document == "docs/contracts/stdlib-sync.md"
  and .documentation.fixture == "tests/runtime/m11-std-sync-doc-001.to"
  and .documentation.command == "scripts/stdlib-sync-doc-check.sh"
  and .documentation.expected_stdout == "sync-doc-ok"
  and .documentation.examples == [
    "ordering-no-deadlock",
    "cleanup-no-poison",
    "explicit-atomic-cas",
    "five-collection-literals",
    "weak-for-vs-snapshot",
    "queue-vs-channel"
  ]
  and .documentation.sections == [
    "surface",
    "ordering",
    "deadlocks",
    "poisoning",
    "cancellation",
    "cleanup",
    "costs",
    "executable-examples"
  ]
  and .implementation.required_follow_ups == []
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-CHANNEL-ASYNC-ITER-001"]
' "$contract" >/dev/null || die "owner contract does not close the documentation record"

for marker in \
    '## Ordering y deadlocks' \
    '## Cancelación, cleanup y ausencia de poisoning' \
    '## Costes y elección queue/channel' \
    '## Ejemplos ejecutables' \
    'ordering-no-deadlock' \
    'cleanup-no-poison' \
    'explicit-atomic-cas' \
    'five-collection-literals' \
    'weak-for-vs-snapshot' \
    'queue-vs-channel' \
    'sync.Array' \
    'sync.Map' \
    'sync.Set' \
    'sync.Stack' \
    'sync.Queue' \
    'one-linearization-coherent-value-collection' \
    'finite-structural-O1' \
    'bounded(0)' \
    'bounded(n)' \
    'unbounded()' \
    'LIFO' \
    'FIFO-MPMC' \
    'std.channel' \
    'STD-SYNC-DOC-001' \
    'm11-std-sync-doc-001.to' \
    'scripts/stdlib-sync-doc-check.sh'; do
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
[[ "$(tr -d '\r\n' < "$fixture_root.stdout")" == "sync-doc-ok" ]] || die "fixture stdout sidecar is not sync-doc-ok"

for function_name in \
    'ordered_mutexes' \
    'cleanup_without_poison' \
    'explicit_atomic_cas' \
    'five_collection_literals' \
    'weak_read_vs_snapshot' \
    'queue_without_backpressure'; do
    grep -Fq "$function_name" "$fixture" || die "fixture misses documented example: $function_name"
done

for literal in \
    'let array: sync.Array[Int] = sync.Array[1, 2]' \
    'let map: sync.Map[String, Int] = sync.Map["a": 1]' \
    'let set: sync.Set[Int] = sync.Set[1, 2]' \
    'let stack: sync.Stack[Int] = sync.Stack[1, 2]' \
    'let queue: sync.Queue[Int] = sync.Queue[1, 2]'; do
    grep -Fq "$literal" "$fixture" || die "fixture misses canonical literal: $literal"
done
grep -Fq 'for item in observed_array' "$fixture" || die "fixture misses weak direct for"
grep -Fq 'observed_array.snapshot()' "$fixture" || die "fixture misses coherent snapshot"
grep -Fq 'nonblocking_queue.dequeue() == none' "$fixture" || die "fixture misses nonblocking queue outcome"

runtime_output="$(cargo run -q -p tondo-cli --locked -- run "$fixture")" || die "executable fixture failed"
[[ "$runtime_output" == "sync-doc-ok" ]] || die "fixture output is not sync-doc-ok: $runtime_output"

echo "std.sync documentation: OK (six executable decisions; queue/channel and snapshot boundaries explicit)"
