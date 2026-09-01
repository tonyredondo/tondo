#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT:-$root/testing/stdlib-sync-collection-conformance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-sync-collection-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.sync collection conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_SYNC_COLLECTION_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-sync-collection-conformance-check.sh

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli -- run \
    tests/runtime/m11-std-sync-collection-conformance-001.to \
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
    [[ "${vm_actual[$index]}" == "${vm_expected[$index]}" ]] || die "hosted VM line $index differs"
done

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime \
    --example sync_collection_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native collection conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[].id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
expected_native="$(jq -c '[.cases[] | .native_expected + {id: .id}]' "$contract")"
jq -e --argjson expected "$expected_native" '
  . == $expected
' <<<"$native_cases" >/dev/null || die "native observations differ from the collection oracle"

CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    sync_collection_direct_iteration_is_value_only_and_suspendable --locked \
    >/dev/null
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler \
    process_host::tests::sync_collection_cursor_preserves_order_horizon_and_reinsertion_boundary --locked \
    >/dev/null

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-sync-collection-conformance-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/sync_collection_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --argjson vm_lines "$vm_lines_json" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-sync-collection-conformance-evidence/1",
      task:"STD-SYNC-COLLECTION-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-sync-collection-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/sync_collection_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-collection-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      comparison:{same_case_ids:true,same_corpus:true,observable_lines:true,array_alias:true,bounds:true,linearization:true,direct_for:true,ordering:true,snapshot:true,stack_queue_non_destructive:true,capability_threads:true,static_rejections:true,suspension_inference:true,generation_horizon:true,cleanup_zero_live_objects:true},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-sync-collection-conformance.json"

echo "std.sync collection conformance: OK (8 shared VM/native cases; report: $evidence_dir/stdlib-sync-collection-conformance.json)"
