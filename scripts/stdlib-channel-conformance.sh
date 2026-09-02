#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT:-$root/testing/stdlib-channel-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-channel-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.channel conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_CHANNEL_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-channel-conformance-check.sh

if [[ "${TONDO_STDLIB_CHANNEL_CONF_ALLOW_DIRTY:-0}" != 1 ]] && [[ -n "$(git status --short)" ]]; then
    die "workspace must be clean; set TONDO_STDLIB_CHANNEL_CONF_ALLOW_DIRTY=1 only for local development"
fi

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
panic_stdout_file="$logs_dir/panic.stdout"
panic_stderr_file="$logs_dir/panic.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-channel-conformance-001.to \
    >"$vm_stdout_file" 2>"$vm_stderr_file"
vm_exit=$?
set -e
expected_exit="$(jq -r '.vm.expected_exit' "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr_file" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
mapfile -t vm_actual < <(tr -d '\r' <"$vm_stdout_file" | sed '/^$/d')
mapfile -t vm_expected < <(jq -r '.vm.expected_stdout[]' "$contract")
[[ "${#vm_actual[@]}" == "${#vm_expected[@]}" ]] || die "hosted VM line count differs"
for index in "${!vm_expected[@]}"; do
    [[ "${vm_actual[$index]}" == "${vm_expected[$index]}" ]] \
        || die "hosted VM line $index differs"
done

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-channel-conformance-panic-001.to \
    >"$panic_stdout_file" 2>"$panic_stderr_file"
panic_exit=$?
set -e
expected_panic_exit="$(jq -r '.vm.panic_expected_exit' "$contract")"
[[ "$panic_exit" == "$expected_panic_exit" ]] || {
    cat "$panic_stderr_file" >&2
    die "panic fixture exited $panic_exit, expected $expected_panic_exit"
}
mapfile -t panic_actual < <(tr -d '\r' <"$panic_stdout_file" | sed '/^$/d')
mapfile -t panic_expected < <(jq -r '.vm.panic_expected_stdout[]' "$contract")
[[ "${panic_actual[*]}" == "${panic_expected[*]}" ]] \
    || die "panic fixture cleanup output differs"

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example channel_shared_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native channel conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[] | select(.id != "channel-shared-conformance") | .id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
expected_native="$(jq -cS '[.cases[] | .native_expected + {id: .id}]' "$contract")"
actual_native="$(jq -cS '[.[] | select(.id != "channel-shared-conformance")]' <<<"$native_cases")"
[[ "$actual_native" == "$expected_native" ]] || die "native observations differ from the channel oracle"
jq -e '.[-1] == {id:"channel-shared-conformance",status:"passed"}' <<<"$native_cases" >/dev/null \
    || die "native probe did not emit its completion marker"

compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    channel_host_ --no-default-features --locked -- --nocapture \
    >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "hosted channel tests failed"; }
native_test_log="$logs_dir/native-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-native-runtime \
    native_channel_ --locked -- --nocapture \
    >"$native_test_log" 2>&1 \
    || { cat "$native_test_log" >&2; die "native channel tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-channel-conformance-001.to | cut -d' ' -f1)"
fixture_panic_sha256="$(sha256sum tests/runtime/m11-std-channel-conformance-panic-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/channel_shared_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
panic_sha256="$(sha256sum "$panic_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d' ' -f1)"
native_test_sha256="$(sha256sum "$native_test_log" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
panic_lines_json="$(printf '%s\n' "${panic_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg fixture_panic_sha256 "$fixture_panic_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg panic_sha256 "$panic_sha256" \
    --arg native_sha256 "$native_sha256" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg native_test_sha256 "$native_test_sha256" \
    --argjson vm_lines "$vm_lines_json" \
    --argjson panic_lines "$panic_lines_json" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-channel-conformance-evidence/1",
      task:"STD-CHANNEL-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-channel-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256),panic_fixture:"tests/runtime/m11-std-channel-conformance-panic-001.to",panic_fixture_sha256:("sha256:" + $fixture_panic_sha256),panic_exit:101,panic_stdout:$panic_lines,panic_status:"passed",panic_log_sha256:("sha256:" + $panic_sha256)},
      hosted_tests:{filter:"channel_host_",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/channel_shared_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-channel-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      native_tests:{filter:"native_channel_",status:"passed",log_sha256:("sha256:" + $native_test_sha256)},
      comparison:{same_case_ids:true,same_corpus:true,observable_lines:true,fifo_commit:true,errors_preserve_payload:true,capacity_errors:true,rendezvous_wakeup:true,terminal_drain:true,close_wakeup:true,panic_cleanup:true,zero_live_endpoints:true,zero_waiters:true,hosted_select:true,native_select_boundary:"private-channel-abi-only"},
      public_boundary:{api_promoted:false,native_aot_lowering:"not-claimed",native_layout:"private"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-channel-conformance.json"

echo "std.channel conformance: OK (8 shared cases; hosted VM/native ABI; report: $evidence_dir/stdlib-channel-conformance.json)"
