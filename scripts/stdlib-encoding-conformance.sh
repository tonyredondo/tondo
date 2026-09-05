#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT:-$root/testing/stdlib-encoding-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-encoding-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.encoding conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-encoding-conformance-check.sh

if [[ "${TONDO_STDLIB_ENCODING_CONF_ALLOW_DIRTY:-0}" != 1 ]] && [[ -n "$(git status --short)" ]]; then
    die "workspace must be clean; set TONDO_STDLIB_ENCODING_CONF_ALLOW_DIRTY=1 only for local development"
fi

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-encoding-conformance-001.to \
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
    --example encoding_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native encoding conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[] | select(.id != "encoding-conformance") | .id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
expected_native="$(jq -cS '[.cases[] | .native_expected + {id: .id}]' "$contract")"
actual_native="$(jq -cS '[.[] | select(.id != "encoding-conformance")]' <<<"$native_cases")"
[[ "$actual_native" == "$expected_native" ]] || die "native observations differ from the encoding oracle"
jq -e '.[-1] == {id:"encoding-conformance",status:"passed"}' <<<"$native_cases" >/dev/null \
    || die "native probe did not emit its completion marker"

stdlib_test_log="$logs_dir/stdlib-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib encoding:: --locked \
    -- --nocapture >"$stdlib_test_log" 2>&1 \
    || { cat "$stdlib_test_log" >&2; die "scalar encoding tests failed"; }
compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler encoding_host_ --locked \
    -- --nocapture >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "hosted encoding tests failed"; }
native_test_log="$logs_dir/native-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-native-runtime native_encoding_ --locked \
    -- --nocapture >"$native_test_log" 2>&1 \
    || { cat "$native_test_log" >&2; die "native encoding tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-encoding-conformance-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/encoding_conformance.rs | cut -d' ' -f1)"
runtime_sha256="$(sha256sum crates/tondo-native-runtime/src/lib.rs | cut -d' ' -f1)"
stdlib_sha256="$(sha256sum crates/tondo-stdlib/src/encoding.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
stdlib_test_sha256="$(sha256sum "$stdlib_test_log" | cut -d' ' -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d' ' -f1)"
native_test_sha256="$(sha256sum "$native_test_log" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg runtime_sha256 "$runtime_sha256" \
    --arg stdlib_sha256 "$stdlib_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --arg stdlib_test_sha256 "$stdlib_test_sha256" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg native_test_sha256 "$native_test_sha256" \
    --argjson vm_lines "$vm_lines_json" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-encoding-conformance-evidence/1",
      task:"STD-ENCODING-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-encoding-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      hosted_tests:{package:"tondo-stdlib",filter:"encoding::",status:"passed",log_sha256:("sha256:" + $stdlib_test_sha256)},
      compiler_tests:{filter:"encoding_host_",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
      native:{runtime:"crates/tondo-native-runtime/src/lib.rs",runtime_sha256:("sha256:" + $runtime_sha256),probe:"crates/tondo-native-runtime/examples/encoding_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),stdlib_oracle:"crates/tondo-stdlib/src/encoding.rs",stdlib_sha256:("sha256:" + $stdlib_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-encoding-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      native_tests:{filter:"native_encoding_",status:"passed",log_sha256:("sha256:" + $native_test_sha256)},
      comparison:{same_case_ids:true,same_corpus:true,observable_lines:true,base64_interoperability:true,hex_policy:true,streaming_chunk_invariance:true,strict_error_kinds:true,strict_error_offsets:true,limit_atomicity:true,terminal_closed:true,zero_limit_empty:true,scalar_oracle_reused:true,simd:"not-measured-no-optimized-route",native_aot_lowering:"not-claimed",zero_live_objects:true},
      public_boundary:{api_promoted:false,native_layout:"private-u64-capabilities",native_aot_lowering:"not-claimed",simd:"not-measured-no-optimized-route"},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-encoding-conformance.json"

echo "std.encoding conformance: OK (6 shared VM/native cases; scalar oracle, streaming, exact errors and cleanup; report: $evidence_dir/stdlib-encoding-conformance.json)"
