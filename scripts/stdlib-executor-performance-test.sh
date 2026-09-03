#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-executor-performance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.executor performance contract tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.protocol.minimum_sample_count = 26' \
    testing/stdlib-executor-performance.json >"$tmp_dir/protocol-count.json"
expect_failure protocol-count \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/protocol-count.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.targets.hosted_vm.probe.sha256 = ("0" * 64)' \
    testing/stdlib-executor-performance.json >"$tmp_dir/hosted-hash.json"
expect_failure hosted-probe-hash \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/hosted-hash.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.targets.native_runtime.probe.sha256 = ("0" * 64)' \
    testing/stdlib-executor-performance.json >"$tmp_dir/native-hash.json"
expect_failure native-probe-hash \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/native-hash.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.workloads[1].id = .workloads[0].id' \
    testing/stdlib-executor-performance.json >"$tmp_dir/duplicate-workload.json"
expect_failure duplicate-workload \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/duplicate-workload.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.strategy.native_aot = "verified"' \
    testing/stdlib-executor-performance.json >"$tmp_dir/aot-claim.json"
expect_failure native-aot-claim \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.workloads |= map(select(.id != "native-drain-4"))' \
    testing/stdlib-executor-performance.json >"$tmp_dir/missing-workload.json"
expect_failure missing-workload \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/missing-workload.json" \
    scripts/stdlib-executor-performance-check.sh

jq '.invariants.target_isolation = "aggregate-all-targets"' \
    testing/stdlib-executor-performance.json >"$tmp_dir/target-isolation.json"
expect_failure target-isolation \
    env TONDO_STDLIB_EXECUTOR_PERF_CONTRACT="$tmp_dir/target-isolation.json" \
    scripts/stdlib-executor-performance-check.sh

bash -n \
    scripts/stdlib-executor-performance-check.sh \
    scripts/stdlib-executor-performance-test.sh \
    scripts/stdlib-executor-performance.sh

scripts/stdlib-executor-performance-check.sh >/dev/null

echo "std.executor performance contract tests: OK (protocol, hashes, workloads, AOT boundary and target isolation reject drift)"
