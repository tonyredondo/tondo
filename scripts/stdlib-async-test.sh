#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-async-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.async owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.surface.inference_by_name = true' testing/stdlib-async.json > "$tmp_dir/name-inference.json"
expect_failure name-inference env TONDO_STDLIB_ASYNC_CONTRACT="$tmp_dir/name-inference.json" scripts/stdlib-async-check.sh

jq '.collect.partial_publication = true' testing/stdlib-async.json > "$tmp_dir/partial-collect.json"
expect_failure partial-collect env TONDO_STDLIB_ASYNC_CONTRACT="$tmp_dir/partial-collect.json" scripts/stdlib-async-check.sh

jq '.iterator.channel_dependency = true' testing/stdlib-async.json > "$tmp_dir/channel-dependency.json"
expect_failure channel-dependency env TONDO_STDLIB_ASYNC_CONTRACT="$tmp_dir/channel-dependency.json" scripts/stdlib-async-check.sh

for marker in \
    'pub type Join[T, E]' \
    'pub type Waiter[T, E]' \
    'pub type Completer[T, E]' \
    'pub fn Waiter.wait(var self): T ! E suspends' \
    'pub fn Completer.complete(var self, value: T): Unit ! AlreadyCompleted' \
    'fn next(mut self): T? suspends' \
    'pub fn AsyncIterator.collect[T](var self, limit: Int): Array[T] ! CollectionError suspends'; do
    grep -Fq "$marker" docs/contracts/stdlib-async.md
done

for marker in \
    'AsyncOneshot' \
    'AsyncWaiterWait' \
    'AsyncCompleterComplete' \
    'AsyncCompleterFail' \
    'AsyncCompleterCancel' \
    'AsyncIteratorNext'; do
    grep -Fq "$marker" crates/tondo-compiler/src/hir/check.rs crates/tondo-compiler/src/hir/lower.rs
done

grep -Fq 'std.async.oneshot' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'std.async.Waiter.wait' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'std.async.Completer.complete' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'std.async.Completer.fail' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'std.async.Completer.cancel' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'AsyncIterator' tests/runtime/m7-async-structured.to crates/tondo-compiler/src/hir/check.rs

jq -e '
  .owner == "std.async"
  and any(.signatures[]; .id == "async-iterator-collect" and .effect == "suspends")
  and .surface.bodyless_requires_effect == true
  and .surface.inference_by_name == false
  and .iterator.channel_dependency == false
' testing/stdlib-async.json >/dev/null

echo "std.async owner tests: OK (negative contract cases and implementation anchors)"
