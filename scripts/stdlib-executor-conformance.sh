#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT:-$root/testing/stdlib-executor-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-executor-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.executor conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_EXECUTOR_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-executor-conformance-check.sh

if [[ "${TONDO_STDLIB_EXECUTOR_CONF_ALLOW_DIRTY:-0}" != 1 ]] && [[ -n "$(git status --short)" ]]; then
    die "workspace must be clean; set TONDO_STDLIB_EXECUTOR_CONF_ALLOW_DIRTY=1 only for local development"
fi

project_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-executor-conformance-project.XXXXXX")"
trap 'rm -rf -- "$project_dir"' EXIT
mkdir -p "$project_dir/src"
cp tests/runtime/m11-std-executor-conformance-001.to "$project_dir/src/main.to"
printf '%s\n' \
    '[package]' \
    'name = "executorconf"' \
    '' \
    '[target]' \
    'capabilities = ["console", "process", "clock", "environment", "filesystem", "threads"]' \
    >"$project_dir/tondo.toml"

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run --project "$project_dir" \
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

negative_stdout_file="$logs_dir/missing-threads.stdout"
negative_stderr_file="$logs_dir/missing-threads.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    check tests/compile-fail/m11-std-executor-conf-missing-threads.to \
    >"$negative_stdout_file" 2>"$negative_stderr_file"
negative_exit=$?
set -e
[[ "$negative_exit" != 0 ]] || die "missing-threads fixture unexpectedly passed"
grep -Fq 'error[E1008]' "$negative_stderr_file" \
    || die "missing-threads fixture did not report E1008"

compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    blocking_pool_requires_an_explicit_threads_target_capability --locked -- --nocapture \
    >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "blockingPool capability compiler test failed"; }

native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example executor_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native executor conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[] | select(.id != "executor-conformance") | .id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
expected_native="$(jq -cS '[.cases[] | .native_expected + {id: .id}]' "$contract")"
actual_native="$(jq -cS '[.[] | select(.id != "executor-conformance")]' <<<"$native_cases")"
[[ "$actual_native" == "$expected_native" ]] || die "native observations differ from executor oracle"
jq -e '.[-1] == {id:"executor-conformance",status:"passed"}' <<<"$native_cases" >/dev/null \
    || die "native probe did not emit its completion marker"

native_test_log="$logs_dir/native-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-native-runtime \
    native_blocking_ --locked -- --nocapture \
    >"$native_test_log" 2>&1 \
    || { cat "$native_test_log" >&2; die "native blocking executor tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-executor-conformance-001.to | cut -d' ' -f1)"
negative_sha256="$(sha256sum tests/compile-fail/m11-std-executor-conf-missing-threads.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/executor_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
negative_sha256_log="$(sha256sum "$negative_stderr_file" | cut -d' ' -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d' ' -f1)"
native_test_sha256="$(sha256sum "$native_test_log" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg negative_sha256 "$negative_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --arg negative_sha256_log "$negative_sha256_log" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg native_test_sha256 "$native_test_sha256" \
    --argjson vm_lines "$vm_lines_json" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-executor-conformance-evidence/1",
      task:"STD-EXEC-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-executor-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/executor_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-executor-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      static_capability:{fixture:"tests/compile-fail/m11-std-executor-conf-missing-threads.to",fixture_sha256:("sha256:" + $negative_sha256),codes:["E1008"],status:"passed",log_sha256:("sha256:" + $negative_sha256_log),driver_test:"blocking_pool_requires_an_explicit_threads_target_capability"},
      hosted_tests:{filter:"blocking_pool_requires_an_explicit_threads_target_capability",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
      native_tests:{filter:"native_blocking_",status:"passed",log_sha256:("sha256:" + $native_test_sha256)},
      comparison:{same_case_ids:true,same_corpus:true,ordered_vm_lines:true,native_observables:true,pool_admission:true,hosted_saturation:true,blocking_transfer:true,safe_cancel_and_drain:true,hosted_actor_fifo:true,hosted_actor_terminal_error:true,static_threads_rejection:true,zero_live_native_handles:true},
      public_boundary:{api_promoted:false,native_runtime:"private-x86_64-linux-opaque-token-lane",native_aot_lowering:"not-claimed",native_layout_abi:false,actor_native_api:"not-claimed"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-executor-conformance.json"

echo "std.executor conformance: OK (8 shared cases; hosted VM/native token bridge; report: $evidence_dir/stdlib-executor-conformance.json)"
