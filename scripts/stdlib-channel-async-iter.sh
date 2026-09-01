#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT:-$root/testing/stdlib-channel-async-iter.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-channel-async-iter-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.channel AsyncIterator implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_CHANNEL_ASYNC_ITER_CONTRACT="$contract" \
    scripts/stdlib-channel-async-iter-check.sh

vm_stdout="$logs_dir/vm.stdout"
vm_stderr="$logs_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-channel-async-iter-001.to \
    >"$vm_stdout" 2>"$vm_stderr"
vm_exit=$?
set -e
expected_exit="$(jq -r '.implementation.fixture.exit' "$contract")"
expected_stdout="$(jq -r '.implementation.fixture.stdout' "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
vm_output="$(tr -d '\r' <"$vm_stdout" | sed '/^$/d' | tail -n 1)"
[[ "$vm_output" == "$expected_stdout" ]] || die "hosted VM output differs: $vm_output"

compiler_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    channel_host_async_iterator --no-default-features --locked -- --nocapture \
    >"$compiler_log" 2>&1 \
    || { cat "$compiler_log" >&2; die "channel host tests failed"; }
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    async_iterator --no-default-features --locked -- --nocapture \
    >>"$compiler_log" 2>&1 \
    || { cat "$compiler_log" >&2; die "AsyncIterator compiler tests failed"; }

negative_stdout="$logs_dir/negative.stdout"
negative_stderr="$logs_dir/negative.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    check tests/compile-fail/m11-std-channel-async-iter-discard.to \
    >"$negative_stdout" 2>"$negative_stderr"
negative_exit=$?
set -e
[[ "$negative_exit" != 0 ]] || die "affine negative unexpectedly passed"
grep -Fq 'error[E1105]' "$negative_stderr" \
    || die "affine negative did not report E1105"

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-channel-async-iter-001.to | cut -d' ' -f1)"
negative_sha256="$(sha256sum tests/compile-fail/m11-std-channel-async-iter-discard.to | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout" | cut -d' ' -f1)"
compiler_sha256="$(sha256sum "$compiler_log" | cut -d' ' -f1)"
negative_log_sha256="$(sha256sum "$negative_stderr" | cut -d' ' -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg negative_sha256 "$negative_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg compiler_sha256 "$compiler_sha256" \
    --arg negative_log_sha256 "$negative_log_sha256" \
    '{
      format:"tondo-stdlib-channel-async-iter-evidence/1",
      task:"STD-CHANNEL-ASYNC-ITER-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-channel-async-iter-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:"channel-async-iter-ok",status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      hosted_tests:{filters:["channel_host_async_iterator","async_iterator"],status:"passed",log_sha256:("sha256:" + $compiler_sha256)},
      negative:{fixture:"tests/compile-fail/m11-std-channel-async-iter-discard.to",fixture_sha256:("sha256:" + $negative_sha256),codes:["E1105"],status:"passed",log_sha256:("sha256:" + $negative_log_sha256)},
      semantics:{one_element_per_next:true,discard_bound:true,early_exit_close:true,cancellation_cleanup:true,generic_collect:true,no_array_intermediate:true},
      public_boundary:{api_promoted:false,native_aot_lowering:"not-claimed",native_abi:"unchanged-parent-channel-abi"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-channel-async-iter.json"

echo "std.channel AsyncIterator implementation: OK (hosted VM; report: $evidence_dir/stdlib-channel-async-iter.json)"
