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
    'required-after-native-gate' \
    'blocking-cooperative-fallback' \
    'pool.submit.accept'; do
    grep -Fq "$marker" testing/stdlib-executor.json
done

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
  and .promotion.next_blocks == ["STD-YAML-001", "DIAG-RUNTIME-001"]
' testing/stdlib-executor.json >/dev/null

echo "std.executor tests: OK (negative contract cases; admission; actors; blocking bridge; lifecycle and capability anchors)"
