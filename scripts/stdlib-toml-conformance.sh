#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_TOML_CONFORMANCE_CONTRACT:-$root/testing/stdlib-toml-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-toml-conformance-logs"
mkdir -p "$logs_dir"

die() {
    echo "std.toml conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_TOML_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-toml-conformance-check.sh

dirty=false
if [[ -n "$(git status --short)" ]]; then
    dirty=true
    [[ "${TONDO_STDLIB_TOML_CONF_ALLOW_DIRTY:-0}" == 1 ]] \
        || die "workspace must be clean; set TONDO_STDLIB_TOML_CONF_ALLOW_DIRTY=1 only for local development"
fi

vm_stdout="$logs_dir/vm.stdout"
vm_stderr="$logs_dir/vm.stderr"
native_stdout="$logs_dir/native.stdout"
native_stderr="$logs_dir/native.stderr"
stdlib_log="$logs_dir/stdlib-tests.log"
model_log="$logs_dir/model-tests.log"

if ! CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-reliability \
    --example toml_conformance_vm --locked >"$vm_stdout" 2>"$vm_stderr"; then
    cat "$vm_stderr" >&2
    die "hosted VM TOML adapter failed"
fi
if ! CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example toml_conformance --locked >"$native_stdout" 2>"$native_stderr"; then
    cat "$native_stderr" >&2
    die "native TOML stdlib process failed"
fi

vm_lines="$(jq -R -s -c 'split("\n") | map(select(length > 0))' <"$vm_stdout")"
expected_lines="$(jq -c '.vm.expected_stdout' "$contract")"
[[ "$vm_lines" == "$expected_lines" ]] || die "VM observable lines differ"
native_cases="$(jq -s -c '.' "$native_stdout")"
jq -e 'length == 7 and .[-1] == {id:"toml-conformance",status:"passed"}' \
    <<<"$native_cases" >/dev/null || die "native completion marker differs"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[0:-1][].id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case IDs differ"
expected_native="$(jq -cS '[.cases[] | .native_expected + {id:.id}]' "$contract")"
actual_native="$(jq -cS '.[0:-1]' <<<"$native_cases")"
[[ "$actual_native" == "$expected_native" ]] || die "native structured observations differ"
native_lines="$(jq -c '[.[0:-1][].line] + ["toml-conformance-ok"]' <<<"$native_cases")"
[[ "$native_lines" == "$vm_lines" ]] || die "VM and native observable lines differ"

if ! CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib toml:: --locked \
    >"$stdlib_log" 2>&1; then
    cat "$stdlib_log" >&2
    die "TOML kernel tests failed"
fi
if ! CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability \
    --test toml_models --locked >"$model_log" 2>&1; then
    cat "$model_log" >&2
    die "independent TOML model tests failed"
fi

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
vm_probe_sha256="$(sha256sum crates/tondo-reliability/examples/toml_conformance_vm.rs | cut -d' ' -f1)"
native_probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/toml_conformance.rs | cut -d' ' -f1)"
stdlib_sha256="$(sha256sum crates/tondo-stdlib/src/toml.rs | cut -d' ' -f1)"
fixture_hashes="$(jq -n --argjson files "$(jq -c '.fixtures.files' "$contract")" '
    $files | map({file:., sha256:null})
')"
for fixture in $(jq -r '.fixtures.files[]' "$contract"); do
    hash="$(sha256sum "testing/stdlib-toml-conformance-fixtures/$fixture" | cut -d' ' -f1)"
    fixture_hashes="$(jq -c --arg file "$fixture" --arg hash "$hash" \
        'map(if .file == $file then .sha256 = ("sha256:" + $hash) else . end)' <<<"$fixture_hashes")"
done
vm_log_sha256="$(sha256sum "$vm_stdout" | cut -d' ' -f1)"
native_log_sha256="$(sha256sum "$native_stdout" | cut -d' ' -f1)"

jq -n \
    --arg revision "$source_revision" \
    --argjson dirty "$dirty" \
    --arg contract_sha256 "$contract_sha256" \
    --arg vm_probe_sha256 "$vm_probe_sha256" \
    --arg native_probe_sha256 "$native_probe_sha256" \
    --arg stdlib_sha256 "$stdlib_sha256" \
    --argjson fixtures "$fixture_hashes" \
    --arg vm_log_sha256 "$vm_log_sha256" \
    --arg native_log_sha256 "$native_log_sha256" \
    --argjson vm_lines "$vm_lines" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-toml-conformance-evidence/1",
      task:"STD-TOML-CONF-001",
      status:"passed",
      source_revision:$revision,
      dirty_tree:$dirty,
      contract_sha256:("sha256:" + $contract_sha256),
      fixtures:$fixtures,
      stdlib_oracle:{source:"crates/tondo-stdlib/src/toml.rs",sha256:("sha256:" + $stdlib_sha256)},
      vm:{probe:"crates/tondo-reliability/examples/toml_conformance_vm.rs",probe_sha256:("sha256:" + $vm_probe_sha256),boundary:"verified-bytecode-test-only-host-callable",status:"passed",stdout:$vm_lines,log_sha256:("sha256:" + $vm_log_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/toml_conformance.rs",probe_sha256:("sha256:" + $native_probe_sha256),status:"passed",abi:"not-implemented",native_aot:"not-claimed",cases:$native_cases,log_sha256:("sha256:" + $native_log_sha256)},
      tests:{stdlib:{filter:"toml::",status:"passed"},model:{filter:"toml_models",status:"passed"}},
      comparison:{same_fixture_bytes:true,same_case_ids:true,exact_observable_lines:true,typed_dynamic:true,canonical_toml_11:true,one_byte_streaming:true,exact_error_path_and_location:true,atomic_limit_rejection:true,zero_live_native_runtime_table_objects:true},
      public_boundary:{compiler_toml_api:"not-implemented",hosted_production_registration:"not-implemented",native_toml_abi:"not-implemented",native_aot_lowering:"not-claimed",simd:"not-measured-no-optimized-route"},
      physical_paths:[],timestamps:false,addresses:[],divergences:[]
    }' >"$evidence_dir/stdlib-toml-conformance.json"

echo "std.toml conformance: OK (6 shared VM-host/native-stdlib cases; report: $evidence_dir/stdlib-toml-conformance.json)"
