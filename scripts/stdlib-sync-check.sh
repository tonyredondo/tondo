#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_CONTRACT:-$root/testing/stdlib-sync.json}"

die() {
    echo "std.sync contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.sync"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-SYNC-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-sync.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B2"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .capability.cooperative_tasks == "available-without-threads"
  and .capability.cross_thread == "requires-threads"
  and .capability.missing_cross_thread == "static-capability-error"
  and .capability.scheduler_blocking == "forbidden"
  and .host.status == "required-after-native-gate"
  and .host.reason == "std.sync needs scheduler parking, wakeups, host atomics and reclamation on VM and native runtimes; cooperative tasks must never block an executor worker"
  and .surface.types == [
    "SyncError = { InvalidCapacity, InvalidParties, ResourceLimit, ReentrantLock, ReentrantInitialization, Broken }",
    "Mutex[T]",
    "MutexGuard[T]",
    "RwLock[T]",
    "ReadGuard[T]",
    "WriteGuard[T]",
    "Condition",
    "Semaphore",
    "Permit",
    "Once[T, E]",
    "Barrier",
    "BarrierRole = { Leader, Follower }",
    "Atomic[T]",
    "MemoryOrder = { Relaxed, Acquire, Release, AcqRel, SeqCst }",
    "CompareExchange[T] = { Exchanged(T), Mismatch(T) }",
    "CollectionError"
  ]
  and ([.surface.signatures[].id] | unique) == [
    "atomic", "atomic-compare-exchange", "atomic-load", "atomic-store", "atomic-swap",
    "barrier", "barrier-wait", "condition", "condition-notify-all", "condition-notify-one",
    "condition-wait", "mutex", "mutex-guard-get", "mutex-guard-get-mut", "mutex-guard-unlock",
    "mutex-lock", "mutex-try-lock", "once", "once-get", "once-get-or-init", "once-is-ready",
    "permit-release", "read-guard-get", "read-guard-unlock", "rw-lock", "rw-read", "rw-try-read",
    "rw-try-write", "rw-write", "semaphore", "semaphore-acquire", "semaphore-try-acquire",
    "write-guard-get", "write-guard-get-mut", "write-guard-unlock"
  ]
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == [
    "barrier-wait", "condition-wait", "mutex-lock", "once-get-or-init", "rw-read", "rw-write", "semaphore-acquire"
  ]
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "required"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == []
  and .surface.order_arguments_are_compile_time_constants == true
  and .ownership.handle_copy == true
  and .ownership.guard_affine == true
  and .ownership.guard_copy == false
  and .ownership.guard_clone == false
  and .ownership.guard_discard == "auto-release-exactly-once"
  and .ownership.permit_affine == true
  and .ownership.permit_copy == false
  and .ownership.permit_clone == false
  and .ownership.permit_discard == "auto-release-exactly-once"
  and .ownership.implicit_poisoning == false
  and .ownership.post_release_use == "compile-error"
  and .ownership.copying_handle_copies_identity == true
  and .locks.mutex.kind == "non-recursive-exclusive"
  and .locks.mutex.reentrant == "SyncError.ReentrantLock"
  and .locks.mutex.cancellation_before_acquire == "unregister-without-lock"
  and .locks.mutex.cancellation_while_held == "release-before-unwind"
  and .locks.mutex.worker_blocking == false
  and .locks.rwlock.readers == "concurrent"
  and .locks.rwlock.writer_exclusion == "exclusive"
  and .locks.rwlock.upgrade == "not-published"
  and .locks.rwlock.downgrade == "not-published"
  and .locks.condition.wait_protocol == "atomically-release-then-register-and-reacquire-before-return"
  and .locks.condition.spurious_wakeups == "hidden-and-rechecked"
  and .locks.condition.cancelled_wait == "reacquire-guard-before-unwind"
  and .locks.condition.notify_one == "wake-oldest-compatible-waiter"
  and .locks.condition.notify_all == "wake-all-registered-waiters"
  and .locks.condition.predicate_loop_required == true
  and .semaphore.capacity == "fixed-positive"
  and .semaphore.initial_permits == "capacity"
  and .semaphore.zero_or_negative == "SyncError.InvalidCapacity"
  and .semaphore.try_acquire == "returns-none-without-suspending"
  and .semaphore.release == "one-permit-exactly-once"
  and .semaphore.cancelled_wait == "unregister-without-consuming-permit"
  and .semaphore.worker_blocking == false
  and .once.states == ["uninitialized", "initializing", "ready"]
  and .once.single_initializer == true
  and .once.success == "publish-once-and-return-ref"
  and .once.declared_error == "return-error-and-reset-to-uninitialized"
  and .once.cancelled_initializer == "reset-to-uninitialized-after-cleanup"
  and .once.panic_initializer == "reset-to-uninitialized-after-cleanup"
  and .once.reentrant_initializer == "SyncError.ReentrantInitialization"
  and .once.mutation_after_ready == "not-published"
  and .barrier.parties == "fixed-positive"
  and .barrier.invalid_parties == "SyncError.InvalidParties"
  and .barrier.generations == "reusable-after-completion"
  and .barrier.last_arriver == "BarrierRole.Leader"
  and .barrier.other_arrivers == "BarrierRole.Follower"
  and .barrier.cancellation == "break-generation-and-wake-all"
  and .barrier.broken_wait == "SyncError.Broken"
  and .barrier.worker_blocking == false
  and .atomics.allowed_T == "Copy + Equatable + Send + Share"
  and .atomics.operations == ["load", "store", "swap", "compareExchange"]
  and .atomics.compare_exchange == "strong-no-spurious-failure"
  and .atomics.mismatch == "returns-observed-value-without-write"
  and .atomics.memory_orders == ["Relaxed", "Acquire", "Release", "AcqRel", "SeqCst"]
  and .atomics.invalid_order == "static-error-for-constant-order"
  and .atomics.default_order == "forbidden"
  and .atomics.wait_notify == "use-Condition-or-channel"
  and .atomics.worker_blocking == false
  and .collections.identities == [
    "sync.Array[T: Copy + Send + Share]",
    "sync.Map[K: Key + Send + Share, V: Copy + Send + Share]",
    "sync.Set[K: Key + Send + Share]",
    "sync.Stack[T: Send + Discard]",
    "sync.Queue[T: Send + Discard]"
  ]
  and .collections.handle_traits == "Copy + Discard + Send + Share"
  and .collections.identity_sharing == "copy-handle-same-state"
  and ([.collections.signatures[].id] | unique) == [
    "array-compare-exchange", "array-empty", "array-get", "array-length", "array-set", "array-snapshot",
    "map-compare-exchange", "map-contains", "map-empty", "map-get", "map-insert", "map-length", "map-remove", "map-snapshot",
    "queue-dequeue", "queue-empty", "queue-enqueue", "queue-length", "queue-peek", "queue-snapshot",
    "set-contains", "set-empty", "set-insert", "set-length", "set-remove", "set-snapshot",
    "stack-empty", "stack-length", "stack-peek", "stack-pop", "stack-push", "stack-snapshot"
  ]
  and all(.collections.signatures[]; (.signature | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.collections.signatures[] | select(.effect == "suspends")] | length) == 22
  and .collections.compare_exchange == "strong-no-spurious-failure"
  and .collections.array.length == "fixed"
  and .collections.array.index == "stable"
  and .collections.array.structural_mutation == "forbidden"
  and .collections.map.order == "insertion-by-linearization"
  and .collections.map.replace_moves_key == false
  and .collections.set.order == "insertion-by-linearization"
  and .collections.stack.order == "LIFO"
  and .collections.queue.order == "FIFO-MPMC"
  and .collections.operations_suspend_under_contention == true
  and .collections.operations_selectable == false
  and .collections.snapshot == "one-linearization-coherent-value-collection"
  and .collections.direct_for.protocol == "AsyncIterator"
  and .collections.direct_for.horizon == "finite-structural-O1"
  and .collections.direct_for.binding == "value-only"
  and .collections.direct_for.post_cursor_insertions == "excluded"
  and .collections.direct_for.lock_held_in_body == false
  and .collections.direct_for.materialization == "forbidden"
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
  and .implementation.status == "verified-compiler-and-hosted-model"
  and .implementation.public_api_promoted == false
  and .implementation.host == "hosted-deterministic-model"
  and .implementation.parking_and_native_bridge == "pending-STD-SYNC-HOST-001"
  and .implementation.fixture == "tests/runtime/m11-std-sync-impl-001.to"
  and .implementation.fixture_stdout == "sync-ok"
  and (.implementation.artifacts | index("crates/tondo-compiler/src/process_host.rs")) != null
' "$contract" >/dev/null || die "invalid machine-readable sync contract"

# Keep the literal assertion readable while retaining an exact check without a
# formatter-dependent trailing-space edge case.
jq -e '.collections.literal_forms == ["sync.Array[...]", "sync.Map[...]", "sync.Set[...]", "sync.Stack[...]", "sync.Queue[...]"]' "$contract" >/dev/null \
    || die "sync collection literal identities drifted"

for path in \
    docs/contracts/stdlib-sync.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-SYNC-001' \
    'pub type Mutex[T]' \
    'pub fn Mutex.lock(ref self): MutexGuard[T] ! SyncError suspends' \
    'pub fn Condition.wait[T](var guard: MutexGuard[T]): MutexGuard[T] suspends' \
    'pub fn Atomic.compareExchange(ref self' \
    'sync.Queue' \
    'auto-release-exactly-once' \
    'one-linearization-coherent-value-collection' \
    'STD-SYNC-HOST-001'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-sync.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-sync.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the sync registry"

echo "std.sync contract: OK (guards; condition/semaphore/once/barrier; explicit atomics; shared collections)"
