#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_SYNC_CONTRACT:-testing/stdlib-sync.json}"
testing_contract="${TONDO_STDLIB_SYNC_TEST_CONTRACT:-testing/stdlib-sync-test.json}"

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

[[ -f "$testing_contract" ]] || {
    echo "std.sync tests: missing testing contract: $testing_contract" >&2
    exit 1
}
tail -c 1 "$testing_contract" | cmp -s <(printf '\n') || {
    echo "std.sync tests: testing contract must end with LF" >&2
    exit 1
}
! grep -nE $'\r|[[:blank:]]$' "$testing_contract" >/dev/null || {
    echo "std.sync tests: testing contract contains CR or trailing whitespace" >&2
    exit 1
}

jq -e '
  .format == "tondo-stdlib-sync-testing/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.sync"
  and .task == "STD-SYNC-TEST-001"
  and .status == "verified"
  and .contract == "testing/stdlib-sync.json"
  and .limits.max_tasks == 64
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 1024
  and .limits.model_seed_count == 4096
  and .limits.fuzz_smoke_runs == 128
  and .model.status == "verified"
  and (.model.sources | index("crates/tondo-reliability/src/sync_model.rs")) != null
  and (.model.sources | index("crates/tondo-reliability/tests/sync_models.rs")) != null
  and (.model.laws | length) == 8
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded state machines with deterministic replay and invariant checks"
  and .vm.status == "verified"
  and .vm.fixture == "tests/runtime/m11-std-sync-test-001.to"
  and .vm.expected_exit == 0
  and .vm.expected_stdout == "sync-test-ok"
  and (.vm.properties | length) == 5
  and .test.status == "verified"
  and (.test.sources | length) >= 5
  and (.test.commands | length) == 6
  and (.test.cases | length) == 8
  and .test.oracle == "runtime output, VM continuation invariants and independent model observations agree"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_sync"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_sync.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_sync/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 1024
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4102
  and .fuzz.smoke.result == "passed"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed-by-hosted-performance"
  and .promotion.runtime_continuation_complete == true
  and .promotion.model_test_fuzz_complete == true
  and .promotion.sanitization_boundary_explicit == true
  and .performance.status == "verified"
  and .performance.contract == "testing/stdlib-sync-performance.json"
  and .performance.document == "docs/contracts/stdlib-sync-performance.md"
  and .performance.target == "tondo-vm-hosted"
  and .performance.workloads == 20
  and .performance.samples_per_workload == 27
  and (.performance.metrics | sort) == [
    "fairness", "latency", "live_handles", "logical_memory_bytes",
    "tail_latency", "throughput"
  ]
  and .performance.report == "target/reliability/evidence/stdlib-sync-performance.json"
  and .performance.command == "scripts/stdlib-sync-performance.sh"
  and .promotion.remaining == [
    "STD-SYNC-COLLECTION-IMPL-001",
    "STD-SYNC-COLLECTION-ITER-001",
    "STD-SYNC-COLLECTION-TEST-001",
    "STD-SYNC-COLLECTION-PERF-001",
    "STD-SYNC-COLLECTION-CONF-001",
    "STD-SYNC-CONF-001",
    "STD-SYNC-DOC-001"
  ]
' "$testing_contract" >/dev/null || {
    echo "std.sync tests: invalid machine-readable TEST contract" >&2
    exit 1
}

TONDO_STDLIB_SYNC_CONTRACT="$contract" scripts/stdlib-sync-check.sh >/dev/null

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

jq '.status = "open"' "$testing_contract" > "$tmp_dir/testing-open.json"
expect_failure testing-status env TONDO_STDLIB_SYNC_TEST_CONTRACT="$tmp_dir/testing-open.json" scripts/stdlib-sync-test.sh

jq '.model.sequence_seeds = 4095' "$testing_contract" > "$tmp_dir/testing-seed-count.json"
expect_failure testing-seed-count env TONDO_STDLIB_SYNC_TEST_CONTRACT="$tmp_dir/testing-seed-count.json" scripts/stdlib-sync-test.sh

jq '.fuzz.step_limit = 2048' "$testing_contract" > "$tmp_dir/testing-fuzz-limit.json"
expect_failure testing-fuzz-limit env TONDO_STDLIB_SYNC_TEST_CONTRACT="$tmp_dir/testing-fuzz-limit.json" scripts/stdlib-sync-test.sh

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

jq '.implementation.status = "pending-after-native-gate"' testing/stdlib-sync.json > "$tmp_dir/stale-implementation.json"
expect_failure stale-implementation env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/stale-implementation.json" scripts/stdlib-sync-check.sh

jq '.implementation.parking_and_native_bridge = "pending-STD-SYNC-HOST-001"' testing/stdlib-sync.json > "$tmp_dir/stale-parking.json"
expect_failure stale-parking env TONDO_STDLIB_SYNC_CONTRACT="$tmp_dir/stale-parking.json" scripts/stdlib-sync-check.sh

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
    'pub fn Condition.wait[T](ref self, var guard: MutexGuard[T]): MutexGuard[T] suspends' \
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
    'verified-scheduler-and-native-bridge' \
    'verified-compiler-hosted-parking-native-bridge' \
    'scheduler-backed-hosted-model' \
    'verified-host-parking-native-atomic-epoch-bridge' \
    'verified-vm-continuation-and-cleanup' \
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
  and .implementation.status == "verified-compiler-hosted-parking-native-bridge"
  and .implementation.host == "scheduler-backed-hosted-model"
  and .implementation.parking_and_native_bridge == "verified-host-parking-native-atomic-epoch-bridge"
  and .implementation.native_atomic_lane == "u64"
  and .implementation.native_parking_signal == "epoch-condvar"
  and .implementation.cooperative_wait == "poll-and-scheduler-park"
  and .implementation.once_initializer_continuation == "verified-vm-continuation-and-cleanup"
  and .implementation.fixture_stdout == "sync-ok"
  and .testing == "testing/stdlib-sync-test.json"
  and .promotion.next_blocks == ["STD-SYNC-COLLECTION-IMPL-001"]
' testing/stdlib-sync.json >/dev/null

scripts/stdlib-sync-collection-frontend-check.sh >/dev/null

for path in \
    tests/runtime/m11-std-sync-impl-001.to \
    tests/runtime/m11-std-sync-impl-001.stdout \
    tests/runtime/m11-std-sync-impl-001.exit; do
    [[ -f "$path" ]] || { echo "std.sync tests: missing implementation evidence path $path" >&2; exit 1; }
done
[[ "$(tr -d '\r\n' < tests/runtime/m11-std-sync-impl-001.exit)" == "0" ]] \
    || { echo "std.sync tests: fixture exit sidecar is not zero" >&2; exit 1; }
[[ "$(tr -d '\r\n' < tests/runtime/m11-std-sync-impl-001.stdout)" == "sync-ok" ]] \
    || { echo "std.sync tests: fixture stdout sidecar is not sync-ok" >&2; exit 1; }

for path in \
    crates/tondo-reliability/src/sync_model.rs \
    crates/tondo-reliability/tests/sync_models.rs \
    fuzz/fuzz_targets/stdlib_sync.rs \
    fuzz/corpus/stdlib_sync/seed \
    scripts/stdlib-sync-fuzz.sh \
    tests/runtime/m11-std-sync-test-001.to \
    testing/stdlib-sync-performance.json \
    docs/contracts/stdlib-sync-performance.md \
    testing/stdlib-sync-collection-frontend.json \
    docs/contracts/stdlib-sync-collection-frontend.md \
    scripts/stdlib-sync-collection-frontend-check.sh \
    scripts/stdlib-sync-collection-frontend-test.sh \
    scripts/stdlib-sync-performance.sh \
    scripts/stdlib-sync-performance-test.sh; do
    [[ -e "$path" ]] || { echo "std.sync tests: missing TEST evidence path $path" >&2; exit 1; }
done
[[ -x scripts/stdlib-sync-fuzz.sh ]] \
    || { echo "std.sync tests: fuzz runner is not executable" >&2; exit 1; }
[[ -x scripts/stdlib-sync-performance.sh ]] \
    || { echo "std.sync tests: performance runner is not executable" >&2; exit 1; }
[[ -x scripts/stdlib-sync-performance-test.sh ]] \
    || { echo "std.sync tests: performance contract runner is not executable" >&2; exit 1; }
[[ -x scripts/stdlib-sync-collection-frontend-check.sh ]] \
    || { echo "std.sync tests: collection frontend checker is not executable" >&2; exit 1; }
[[ -x scripts/stdlib-sync-collection-frontend-test.sh ]] \
    || { echo "std.sync tests: collection frontend contract test is not executable" >&2; exit 1; }
[[ -s fuzz/corpus/stdlib_sync/seed ]] \
    || { echo "std.sync tests: fuzz corpus is empty" >&2; exit 1; }

for marker in \
    'OnceContinuation' \
    'RuntimeOnceState' \
    'OperationResult::OnceInit' \
    'TaskWait::Once' \
    'finish_once_initializer_unwind' \
    'publish_once' \
    'MAX_FUZZ_INPUT_BYTES' \
    'MAX_FUZZ_STEPS'; do
    grep -Fq "$marker" crates/tondo-vm/src/runtime/execute.rs crates/tondo-reliability/src/sync_model.rs \
        || { echo "std.sync tests: missing implementation/model anchor $marker" >&2; exit 1; }
done

cargo test -p tondo-compiler process_host::tests::sync_ --locked >/dev/null
cargo test -p tondo-vm --lib --locked >/dev/null
cargo test -p tondo-native-runtime native_sync_ --locked >/dev/null
cargo test -p tondo-reliability --test sync_models --locked >/dev/null
cargo check --manifest-path fuzz/Cargo.toml --bin stdlib_sync --locked >/dev/null
runtime_output="$(cargo run -q -p tondo-cli -- run tests/runtime/m11-std-sync-impl-001.to)"
[[ "$runtime_output" == "sync-ok" ]] \
    || { echo "std.sync tests: runtime fixture produced unexpected output: $runtime_output" >&2; exit 1; }
test_runtime_output="$(cargo run -q -p tondo-cli -- run tests/runtime/m11-std-sync-test-001.to)"
[[ "$test_runtime_output" == "sync-test-ok" ]] \
    || { echo "std.sync tests: TEST runtime fixture produced unexpected output: $test_runtime_output" >&2; exit 1; }

echo "std.sync tests: OK (negative contracts; models; VM Once continuation; bounded fuzz target; teardown anchors)"
