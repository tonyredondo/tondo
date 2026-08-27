#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_NATIVE_THREAD_CONTRACT:-$root/testing/native-thread.json}"

die() {
    echo "native thread: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with one LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  def unique_values: length == (unique | length);
  .format == "tondo-native-thread/1"
  and .owner == "toolchain.native_runtime"
  and .edition == "0.1"
  and .task == "NATIVE-THREAD-001"
  and .status == "closed"
  and .implementation.runtime == "crates/tondo-native-runtime/src/lib.rs"
  and .implementation.native_adapter == "tools/native-evaluation/src/main.rs"
  and .implementation.evidence == "target/reliability/evidence/native-evaluation-runner.json"
  and .implementation.report_field == "native_thread_runs"
  and .lane.source == "spawn thread call()"
  and .lane.join == "same-opaque-Join-state-machine"
  and .lane.vm == "single-cooperative-queue-with-deterministic-state"
  and .lane.native == "one-os-worker-lane-per-thread-spawn"
  and .lane.adapter_body_boundary == "lowered-value-is-evaluated-before-current-adapter-handoff"
  and .lane.deferred_body_lowering == "closed-for-direct-task-calls-by-NATIVE-002"
  and .worker.states == ["starting", "running", "completed", "cancelled"]
  and .worker.identity == "logical-handle-sequence; physical-thread-id-never-exposed"
  and .worker.distinct_thread == "worker-thread-id-differs-from-spawner"
  and .worker.completion == "worker-completes-before-Join-or-await-observes-value"
  and .worker.cleanup == "worker-signal-terminal-transition-is-idempotent-and-detached-without-leak"
  and .abi.status == "tondo_rt_thread_worker_status(task)"
  and .abi.runs == "tondo_rt_thread_worker_runs(task)"
  and .abi.distinct == "tondo_rt_thread_worker_distinct(task)"
  and .abi.wait == "tondo_rt_thread_worker_wait(task)"
  and .abi.invalid_handle == "u64-max"
  and .abi.status_codes == {starting: 0, running: 1, completed: 2, cancelled: 3}
  and (.corpus.native_cases | length == 6 and unique_values)
  and (.corpus.native_cases | index("select-thread-join") != null)
  and (.invariants | length == 6 and unique_values)
  and (.negative_cases | length == 8 and unique_values)
  and .next_blocks == ["NATIVE-STD-CORE-001"]
' "$contract" >/dev/null || die "invalid machine-readable contract"

for path in \
    crates/tondo-native-runtime/src/lib.rs \
    tools/native-evaluation/src/main.rs \
    testing/native-evaluation-runner.json \
    docs/contracts/native-thread.md \
    docs/contracts/native-abi.md \
    docs/contracts/native-evaluation.md; do
    [[ -f "$root/$path" ]] || die "missing native thread input: $path"
done

for symbol in \
    tondo_rt_thread_spawn \
    tondo_rt_thread_worker_status \
    tondo_rt_thread_worker_runs \
    tondo_rt_thread_worker_distinct \
    tondo_rt_thread_worker_wait; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "runtime symbol is missing: $symbol"
    grep -Fq "$symbol" "$root/tools/native-evaluation/src/main.rs" \
        || die "native adapter symbol is missing: $symbol"
done

grep -Fq 'std::thread::Builder' "$root/crates/tondo-native-runtime/src/lib.rs" \
    || die "runtime does not create an OS worker"
grep -Fq 'pthread_create' "$root/tools/native-evaluation/src/main.rs" \
    || die "native harness does not create an OS worker"
grep -Fq 'pthread_join' "$root/tools/native-evaluation/src/main.rs" \
    || die "native harness does not join its worker"
grep -Fq 'arg("-pthread")' "$root/tools/native-evaluation/src/main.rs" \
    || die "native harness is not linked with pthread support"
grep -Fq 'native_thread_runs' "$root/scripts/native-evaluation-runner.sh" \
    || die "native runner does not validate thread evidence"
grep -Fq 'thread-worker-cancel' "$root/tools/native-evaluation/src/main.rs" \
    || die "native corpus does not exercise cancellation"

echo "native thread contract: OK (OS worker lifecycle, Join barrier, cancellation and path-free identity)"
