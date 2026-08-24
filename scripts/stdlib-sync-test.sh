#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-sync-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.sync tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.locks.mutex.reentrant = "deadlock"' testing/stdlib-sync.json > "$tmp_dir/reentrant.json"
expect_failure reentrant-lock env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/reentrant.json" scripts/stdlib-sync-check.sh

jq '.ownership.guard_discard = "forbidden"' testing/stdlib-sync.json > "$tmp_dir/guard-discard.json"
expect_failure guard-cleanup env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/guard-discard.json" scripts/stdlib-sync-check.sh

jq '.locks.condition.wait_protocol = "unlock-then-register"' testing/stdlib-sync.json > "$tmp_dir/condition-race.json"
expect_failure condition-registration env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/condition-race.json" scripts/stdlib-sync-check.sh

jq '.semaphore.capacity = "zero-allowed"' testing/stdlib-sync.json > "$tmp_dir/semaphore-capacity.json"
expect_failure semaphore-capacity env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/semaphore-capacity.json" scripts/stdlib-sync-check.sh

jq '.once.declared_error = "sticky-failure"' testing/stdlib-sync.json > "$tmp_dir/once-error.json"
expect_failure once-error-reset env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/once-error.json" scripts/stdlib-sync-check.sh

jq '.barrier.cancellation = "ignore-cancellation"' testing/stdlib-sync.json > "$tmp_dir/barrier-cancel.json"
expect_failure barrier-cancellation env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/barrier-cancel.json" scripts/stdlib-sync-check.sh

jq '.atomics.default_order = "Relaxed"' testing/stdlib-sync.json > "$tmp_dir/atomic-default.json"
expect_failure atomic-default-order env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/atomic-default.json" scripts/stdlib-sync-check.sh

jq '.collections.snapshot = "best-effort-mixed-state"' testing/stdlib-sync.json > "$tmp_dir/mixed-snapshot.json"
expect_failure coherent-snapshot env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/mixed-snapshot.json" scripts/stdlib-sync-check.sh

jq '.collections.direct_for.binding = "ref-or-value"' testing/stdlib-sync.json > "$tmp_dir/borrowed-for.json"
expect_failure borrowed-for env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/borrowed-for.json" scripts/stdlib-sync-check.sh

jq '.surface.selectable_operations = ["mutex-lock"]' testing/stdlib-sync.json > "$tmp_dir/selectable-lock.json"
expect_failure selectable-lock env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/selectable-lock.json" scripts/stdlib-sync-check.sh

for marker in \
    'pub enum SyncError' \
    'pub type Mutex[T]' \
    'pub type RwLock[T]' \
    'pub type MutexGuard[T]' \
    'pub type Permit' \
    'pub type Once[T, E]' \
    'pub enum MemoryOrder' \
    'pub fn mutex[T: Send](value: T): Mutex[T] ! SyncError' \
    'pub fn Mutex.lock(ref self): MutexGuard[T] ! SyncError suspends' \
    'pub fn RwLock.read(ref self): ReadGuard[T] ! SyncError suspends' \
    'pub fn Condition.wait[T](var guard: MutexGuard[T]): MutexGuard[T] suspends' \
    'pub fn Semaphore.acquire(ref self): Permit suspends' \
    'pub fn Once.getOrInit(ref self, init: fn(): T ! E suspends): ref T ! E suspends' \
    'pub fn Barrier.wait(ref self): BarrierRole ! SyncError suspends' \
    'pub fn Atomic.compareExchange(ref self' \
    'sync.Array[...]' \
    'pub fn sync.Array.get(ref self, index: Int): T? suspends' \
    'pub fn sync.Map.insert(ref self, key: K, value: V): V? ! CollectionError suspends' \
    'pub fn sync.Set.snapshot(ref self): Set[K] ! CollectionError suspends' \
    'pub fn sync.Stack.pop(ref self): T? suspends' \
    'pub fn sync.Queue.dequeue(ref self): T? suspends' \
    'sync.Queue[...]'; do
    grep -Fq "$marker" docs/contracts/stdlib-sync.md
done

for marker in \
    'auto-release-exactly-once' \
    'hidden-and-rechecked' \
    'strong-no-spurious-failure' \
    'one-linearization-coherent-value-collection' \
    'finite-structural-O1' \
    'required-after-native-gate' \
    'WaitGroup' \
    'implicit-poisoning'; do
    grep -Fq "$marker" testing/stdlib-sync.json
done

jq -e '
  .task == "STD-SYNC-001"
  and .surface.selectable_operations == []
  and .ownership.implicit_poisoning == false
  and .locks.condition.spurious_wakeups == "hidden-and-rechecked"
  and .atomics.compare_exchange == "strong-no-spurious-failure"
  and .collections.snapshot == "one-linearization-coherent-value-collection"
  and .collections.direct_for.binding == "value-only"
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["STD-YAML-001"]
' testing/stdlib-sync.json >/dev/null

echo "std.sync tests: OK (negative contract cases; cleanup; ordering; collections; promotion anchors)"
