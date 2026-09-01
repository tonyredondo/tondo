#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-channel-async-iter.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.channel AsyncIterator tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.surface.bound = "T: Copy"' testing/stdlib-channel-async-iter.json \
    >"$tmp_dir/wrong-bound.json"
expect_failure wrong-bound \
    env TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$tmp_dir/wrong-bound.json" \
    scripts/stdlib-channel-async-iter-check.sh

jq '.surface.for_await = "allowed"' testing/stdlib-channel-async-iter.json \
    >"$tmp_dir/for-await.json"
expect_failure for-await \
    env TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$tmp_dir/for-await.json" \
    scripts/stdlib-channel-async-iter-check.sh

jq '.runtime.native_aot_lowering = "verified"' testing/stdlib-channel-async-iter.json \
    >"$tmp_dir/aot-claim.json"
expect_failure aot-claim \
    env TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$tmp_dir/aot-claim.json" \
    scripts/stdlib-channel-async-iter-check.sh

jq '.promotion.next_blocks = ["STD-CHANNEL-ASYNC-ITER-001"]' \
    testing/stdlib-channel-async-iter.json >"$tmp_dir/stale-promotion.json"
expect_failure stale-promotion \
    env TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$tmp_dir/stale-promotion.json" \
    scripts/stdlib-channel-async-iter-check.sh

jq '.negative_cases += ["iterator-without-discard-bound"]' \
    testing/stdlib-channel-async-iter.json >"$tmp_dir/duplicate-negative.json"
expect_failure duplicate-negative \
    env TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$tmp_dir/duplicate-negative.json" \
    scripts/stdlib-channel-async-iter-check.sh

bash -n scripts/stdlib-channel-async-iter-check.sh \
    scripts/stdlib-channel-async-iter-test.sh \
    scripts/stdlib-channel-async-iter.sh
scripts/stdlib-channel-async-iter-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-target}"
compiler_log="$tmp_dir/compiler.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    channel_host_async_iterator --no-default-features --locked -- --nocapture \
    >"$compiler_log" 2>&1 || { cat "$compiler_log" >&2; exit 1; }
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    async_iterator --no-default-features --locked -- --nocapture \
    >>"$compiler_log" 2>&1 || { cat "$compiler_log" >&2; exit 1; }

vm_stdout="$tmp_dir/vm.stdout"
vm_stderr="$tmp_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-channel-async-iter-001.to \
    >"$vm_stdout" 2>"$vm_stderr"
vm_exit=$?
set -e
[[ "$vm_exit" == 0 ]] || { cat "$vm_stderr" >&2; exit 1; }
vm_output="$(tr -d '\r' <"$vm_stdout" | sed '/^$/d' | tail -n 1)"
[[ "$vm_output" == "channel-async-iter-ok" ]] \
    || { echo "unexpected VM output: $vm_output" >&2; exit 1; }

negative_stderr="$tmp_dir/negative.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    check tests/compile-fail/m11-std-channel-async-iter-discard.to \
    >"$tmp_dir/negative.stdout" 2>"$negative_stderr"
negative_exit=$?
set -e
[[ "$negative_exit" != 0 ]] || { echo "affine negative unexpectedly passed" >&2; exit 1; }
grep -Fq 'error[E1105]' "$negative_stderr" \
    || { cat "$negative_stderr" >&2; exit 1; }

echo "std.channel AsyncIterator tests: OK (contract negatives; compiler; hosted VM; affine E1105)"
