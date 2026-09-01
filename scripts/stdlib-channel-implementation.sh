#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_CONTRACT:-$root/testing/stdlib-channel.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-channel-implementation-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.channel implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_CHANNEL_CONTRACT="$contract" scripts/stdlib-channel-implementation-check.sh

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-channel-impl-001.to \
    >"$vm_stdout_file" 2>"$vm_stderr_file"
vm_exit=$?
set -e
expected_exit="$(jq -r '.implementation.fixture.exit' "$contract")"
expected_stdout="$(jq -r '.implementation.fixture.stdout' "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr_file" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
vm_stdout="$(tr -d '\r' <"$vm_stdout_file" | sed '/^$/d' | tail -n 1)"
[[ "$vm_stdout" == "$expected_stdout" ]] || die "hosted VM output differs: $vm_stdout"

compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    channel_host --no-default-features --locked -- --nocapture \
    >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "hosted channel tests failed"; }

native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example channel_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native channel probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
jq -e '
  length == 4
  and .[0] == {id:"bounded-fifo",status:"passed",full_preserves_payload:true,empty:true,closed:true,cleanup:true}
  and .[1] == {id:"rendezvous-wakeup",status:"passed",fifo_registration:true,close_wakes:true,cleanup:true}
  and .[2] == {id:"terminal-drain",status:"passed",pending_fifo:true,sender_closed:true,cleanup:true}
  and .[3] == {id:"channel-conformance",status:"passed"}
' <<<"$native_cases" >/dev/null || die "native observations violate the channel oracle"

native_test_log="$logs_dir/native-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-native-runtime \
    native_channel_ --locked -- --nocapture \
    >"$native_test_log" 2>&1 \
    || { cat "$native_test_log" >&2; die "native channel tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-channel-impl-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/channel_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d' ' -f1)"
native_test_sha256="$(sha256sum "$native_test_log" | cut -d' ' -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg native_test_sha256 "$native_test_sha256" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-channel-implementation-evidence/1",
      task:"STD-CHANNEL-IMPL-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-channel-impl-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:"channel-ok",status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      hosted_tests:{filter:"channel_host",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/channel_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-channel-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      native_tests:{filter:"native_channel_",status:"passed",log_sha256:("sha256:" + $native_test_sha256)},
      comparison:{fifo_commit:true,bounded_backpressure:true,unbounded_resource_limit:true,rendezvous_wakeup:true,selectable_send:true,selectable_receive:true,cancellation_cleanup:true,terminal_drain:true,failed_send_preserves_payload:true,no_detached_waiters:true},
      public_boundary:{api_promoted:false,native_aot_lowering:"not-claimed",native_layout:"private"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-channel-implementation.json"

echo "std.channel implementation: OK (hosted VM and private native ABI; report: $evidence_dir/stdlib-channel-implementation.json)"
