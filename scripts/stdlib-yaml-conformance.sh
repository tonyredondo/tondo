#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT:-$root/testing/stdlib-yaml-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-yaml-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.yaml conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-yaml-conformance-check.sh

if [[ "${TONDO_STDLIB_YAML_CONF_ALLOW_DIRTY:-0}" != 1 ]] && [[ -n "$(git status --short)" ]]; then
    die "workspace must be clean; set TONDO_STDLIB_YAML_CONF_ALLOW_DIRTY=1 only for local development"
fi

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-yaml-conformance-001.to \
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
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example yaml_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native YAML conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[] | select(.id != "yaml-conformance") | .id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case IDs differ from shared corpus"
expected_native="$(jq -cS '[.cases[] | .native_expected + {id: .id}]' "$contract")"
actual_native="$(jq -cS '[.[] | select(.id != "yaml-conformance")]' <<<"$native_cases")"
[[ "$actual_native" == "$expected_native" ]] || die "native observations differ from the YAML oracle"
jq -e '.[-1] == {id:"yaml-conformance",status:"passed"}' <<<"$native_cases" >/dev/null \
    || die "native probe did not emit its completion marker"

stdlib_test_log="$logs_dir/stdlib-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib yaml:: --locked \
    -- --nocapture >"$stdlib_test_log" 2>&1 \
    || { cat "$stdlib_test_log" >&2; die "YAML scalar tests failed"; }
compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler yaml_bootstrap_surface_ --locked \
    -- --nocapture >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "YAML checker tests failed"; }
host_test_log="$logs_dir/host-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    'process_host::tests::yaml_' --locked -- --nocapture \
    >"$host_test_log" 2>&1 \
    || { cat "$host_test_log" >&2; die "hosted YAML tests failed"; }
vm_test_log="$logs_dir/vm-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-vm --locked \
    default_host_and_closed_runtime_helpers_have_explicit_boundaries -- --nocapture \
    >"$vm_test_log" 2>&1 \
    || { cat "$vm_test_log" >&2; die "VM host-kind tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-yaml-conformance-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/yaml_conformance.rs | cut -d' ' -f1)"
stdlib_sha256="$(sha256sum crates/tondo-stdlib/src/yaml.rs | cut -d' ' -f1)"
checker_sha256="$(sha256sum crates/tondo-compiler/src/hir/check.rs | cut -d' ' -f1)"
lower_sha256="$(sha256sum crates/tondo-compiler/src/hir/lower.rs | cut -d' ' -f1)"
host_sha256="$(sha256sum crates/tondo-compiler/src/process_host.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
stdlib_test_sha256="$(sha256sum "$stdlib_test_log" | cut -d' ' -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d' ' -f1)"
host_test_sha256="$(sha256sum "$host_test_log" | cut -d' ' -f1)"
vm_test_sha256="$(sha256sum "$vm_test_log" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg stdlib_sha256 "$stdlib_sha256" \
    --arg checker_sha256 "$checker_sha256" \
    --arg lower_sha256 "$lower_sha256" \
    --arg host_sha256 "$host_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --arg stdlib_test_sha256 "$stdlib_test_sha256" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg host_test_sha256 "$host_test_sha256" \
    --arg vm_test_sha256 "$vm_test_sha256" \
    --argjson vm_lines "$vm_lines_json" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-yaml-conformance-evidence/1",
      task:"STD-YAML-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-yaml-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      hosted_tests:{package:"tondo-stdlib",filter:"yaml::",status:"passed",log_sha256:("sha256:" + $stdlib_test_sha256)},
      compiler_tests:{filter:"yaml_bootstrap_surface_",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
      host_tests:{filter:"process_host::tests::yaml_",status:"passed",log_sha256:("sha256:" + $host_test_sha256)},
      vm_tests:{filter:"default_host_and_closed_runtime_helpers_have_explicit_boundaries",status:"passed",log_sha256:("sha256:" + $vm_test_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/yaml_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),stdlib_oracle:"crates/tondo-stdlib/src/yaml.rs",stdlib_sha256:("sha256:" + $stdlib_sha256),compiler_checker:"crates/tondo-compiler/src/hir/check.rs",checker_sha256:("sha256:" + $checker_sha256),hir_lowering:"crates/tondo-compiler/src/hir/lower.rs",lower_sha256:("sha256:" + $lower_sha256),host_bridge:"crates/tondo-compiler/src/process_host.rs",host_sha256:("sha256:" + $host_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-yaml-lowering",abi:"not-implemented",native_aot:"not-claimed",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      comparison:{same_case_ids:true,same_corpus:true,observable_lines:true,dynamic_typed_round_trip:true,canonical_yaml:true,binary_base64:true,one_byte_streaming:true,event_count:true,exact_error_path:true,exact_error_location:true,limit_atomicity:true,terminal_closed:true,scalar_oracle_reused:true,simd:"not-measured-no-optimized-route",native_aot_lowering:"not-claimed",zero_live_runtime_table_objects:true},
      public_boundary:{api_promoted:false,native_yaml_abi:"not-implemented",native_aot_lowering:"not-claimed",simd:"not-measured-no-optimized-route"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-yaml-conformance.json"

echo "std.yaml conformance: OK (6 shared VM/native-stdlib cases; typed/dynamic, streaming, errors, limits and explicit AOT boundary; report: $evidence_dir/stdlib-yaml-conformance.json)"
