#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-executor-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.executor tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.capacity.zero_queue = "buffered"' testing/stdlib-executor.json > "$tmp_dir/not-zero-queue.json"
expect_failure zero-queue env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/not-zero-queue.json" scripts/stdlib-executor-check.sh

jq '.capability.blocking_pool = "available-without-threads"' testing/stdlib-executor.json > "$tmp_dir/missing-capability.json"
expect_failure blocking-capability env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/missing-capability.json" scripts/stdlib-executor-check.sh

jq '.surface.selectable_operations = ["pool-submit"]' testing/stdlib-executor.json > "$tmp_dir/selectable-submit.json"
expect_failure selectable-submit env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/selectable-submit.json" scripts/stdlib-executor-check.sh

jq '.ownership.job_on_rejection = "discarded"' testing/stdlib-executor.json > "$tmp_dir/lost-job.json"
expect_failure lost-job env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/lost-job.json" scripts/stdlib-executor-check.sh

jq '.blocking.job_effect = "suspends"' testing/stdlib-executor.json > "$tmp_dir/suspendible-blocking-job.json"
expect_failure suspendible-blocking-job env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/suspendible-blocking-job.json" scripts/stdlib-executor-check.sh

jq '.blocking.cancel_running = "force-kill"' testing/stdlib-executor.json > "$tmp_dir/force-kill.json"
expect_failure force-kill env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/force-kill.json" scripts/stdlib-executor-check.sh

jq '.actor.handler_concurrency = "parallel"' testing/stdlib-executor.json > "$tmp_dir/parallel-actor.json"
expect_failure parallel-actor env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/parallel-actor.json" scripts/stdlib-executor-check.sh

jq '.lifecycle.drain_before_success = false' testing/stdlib-executor.json > "$tmp_dir/early-success.json"
expect_failure early-success env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/early-success.json" scripts/stdlib-executor-check.sh

jq '.diagnostics.payloads = "included-by-default"' testing/stdlib-executor.json > "$tmp_dir/payload-leak.json"
expect_failure payload-leak env TONDO_STDLIB_EXECUTOR_CONTRACT="$tmp_dir/payload-leak.json" scripts/stdlib-executor-check.sh

for marker in \
    'pub enum ExecutorError' \
    'pub enum SubmitError' \
    'pub enum ActorSendError[M]' \
    'pub fn pool(workers: Int, capacity: Int): Pool ! ExecutorError' \
    'pub fn blockingPool(workers: Int, capacity: Int): BlockingPool ! ExecutorError' \
    'pub fn Pool.trySubmit[T, E]' \
    'pub fn Actor.ref(ref self): ActorRef[M]' \
    'pub fn Pool.shutdown(self): Unit suspends' \
    'pub fn Pool.cancel(self): Unit suspends' \
    'pub fn Actor.stop(self): Unit ! E suspends' \
    'pub fn BlockingPool.shutdown(self): Unit suspends'; do
    grep -Fq "$marker" docs/contracts/stdlib-executor.md
done

for marker in \
    'FIFO-admission' \
    'moves-on-admission-commit' \
    'caller-retains-job' \
    'one-message-at-a-time' \
    'close-mailbox-cancel-handler-drain-cleanup' \
    'without-blocking-cooperative-worker' \
    'verified-hosted-and-target-qualified-native-bridge' \
    'blocking-cooperative-fallback' \
    'pool.submit.accept'; do
    grep -Fq "$marker" testing/stdlib-executor.json
done

for path in \
    crates/tondo-reliability/src/executor_model.rs \
    crates/tondo-reliability/tests/models.rs \
    fuzz/fuzz_targets/stdlib_executor.rs \
    fuzz/corpus/stdlib_executor/seed \
    scripts/stdlib-executor-fuzz.sh \
    testing/stdlib-executor-test.json; do
    [[ -e "$path" ]] || {
        echo "std.executor tests: missing evidence path $path" >&2
        exit 1
    }
done
[[ -x scripts/stdlib-executor-fuzz.sh ]] || {
    echo "std.executor tests: fuzz runner is not executable" >&2
    exit 1
}
grep -Fq 'name = "stdlib_executor"' fuzz/Cargo.toml

jq -e '
  .format == "tondo-stdlib-executor-testing/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.executor"
  and .task == "STD-EXEC-TEST-001"
  and .status == "verified"
  and .contract == "testing/stdlib-executor.json"
  and .limits.max_workers == 8
  and .limits.max_capacity == 16
  and .limits.max_jobs == 64
  and .limits.max_actor_messages == 64
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 1024
  and .limits.model_seed_count == 4096
  and .limits.stress_workers == 4
  and .limits.stress_jobs == 32
  and .model.status == "verified"
  and (.model.laws | type == "array" and length >= 10)
  and .model.sequence_seeds == 4096
  and .test.status == "verified"
  and (.test.sources | type == "array" and length > 0)
  and (.test.commands | type == "array" and length > 0)
  and (.test.cases | type == "array" and length >= 10)
  and .test.runtime_output == "executor-ok"
  and .test.stress.status == "verified"
  and .test.stress.workers == 4
  and .test.stress.jobs == 32
  and .fuzz.target == "stdlib_executor"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_executor.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_executor/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 1024
  and .fuzz.status == "verified"
  and .fuzz.smoke.result == "passed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.remaining[0] == "STD-EXEC-PERF-001"
' testing/stdlib-executor-test.json >/dev/null

jq -e '
  .task == "STD-EXEC-001"
  and .implementation.public_api_promoted == false
  and .pool.starvation == "forbidden-by-default-under-cooperative-scheduler"
  and .blocking.cancel_running == "wait-for-safe-host-return"
  and .actor.handler_concurrency == "one-message-at-a-time"
  and .actor.pending_messages == "discarded-under-M-Discard"
  and .lifecycle.drain_before_success == true
' testing/stdlib-executor.json >/dev/null

cargo test -q -p tondo-reliability --test models executor_model::tests:: --locked
cargo test -q -p tondo-reliability --test models executor_model_sequences_are_bounded_replayable_and_cleanup_complete --locked
cargo test -q -p tondo-vm --lib runtime::execute::tests::blocking_bridge_ --locked
cargo test -q -p tondo-vm --lib runtime::execute::tests::blocking_worker_host_ --locked

jq -e '
  .task == "STD-EXEC-001"
  and .surface.selectable_operations == ["actor-send"]
  and .capacity.zero_queue == "running-slots-only"
  and .ownership.job_transfer == "moves-on-admission-commit"
  and .blocking.job_effect == "non-suspendible"
  and .blocking.force_kill == false
  and .actor.handler_concurrency == "one-message-at-a-time"
  and .actor.send_select_rollback == "unregister-with-message-still-owned-by-sender"
  and .lifecycle.drain_before_success == true
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["DIAG-RUNTIME-001"]
' testing/stdlib-executor.json >/dev/null

echo "std.executor tests: OK (negative contract cases; admission; actors; blocking bridge; lifecycle and capability anchors)"
