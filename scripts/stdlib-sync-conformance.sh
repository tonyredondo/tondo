#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE[0]")/.." && pwd)"
cd "$root"
contract="$(printenv TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT || printf '%s/testing/stdlib-sync-conformance.json' "$root")"
target_dir="$(printenv CARGO_TARGET_DIR || printf target)"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$(printenv TONDO_STDLIB_EVIDENCE_DIR || printf '%s/reliability/evidence' "$target_dir")"
logs_dir="$evidence_dir/stdlib-sync-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.sync conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_SYNC_CONFORMANCE_CONTRACT="$contract" scripts/stdlib-sync-conformance-check.sh

TONDO_STDLIB_EVIDENCE_DIR="$evidence_dir" CARGO_TARGET_DIR="$target_dir" scripts/stdlib-sync-collection-conformance.sh >/dev/null
collection_report="$evidence_dir/stdlib-sync-collection-conformance.json"
jq -e '
  .task == "STD-SYNC-COLLECTION-CONF-001"
  and .status == "passed"
  and .comparison.same_case_ids == true
  and .comparison.cleanup_zero_live_objects == true
  and (.physical_paths | length == 0)
  and .timestamps == false
' "$collection_report" >/dev/null || die "collection conformance child report is not verified"

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli -- run tests/runtime/m11-std-sync-conformance-001.to >"$vm_stdout_file" 2>"$vm_stderr_file"
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

CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler thread_spawn_requires_an_explicit_threads_target_capability --locked >/dev/null

set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-native-runtime --example sync_conformance --locked >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native sync conformance probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_native="$(jq -c '[.cases[].native_expected]' "$contract")"
actual_ids="$(jq -c '[.[].id]' <<<"$native_cases")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
jq -e --argjson expected "$expected_native" '. == $expected' <<<"$native_cases" >/dev/null || die "native observations differ from the sync oracle"

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-sync-conformance-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/sync_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
collection_sha256="$(sha256sum "$collection_report" | cut -d' ' -f1)"
vm_lines_json="$(printf '%s\n' "${vm_actual[@]}" | jq -R -s 'split("\n") | map(select(length > 0))')"
jq -n --arg revision "$source_revision" --arg contract_sha256 "$contract_sha256" --arg fixture_sha256 "$fixture_sha256" --arg probe_sha256 "$probe_sha256" --arg vm_sha256 "$vm_sha256" --arg native_sha256 "$native_sha256" --arg collection_sha256 "$collection_sha256" --argjson vm_lines "$vm_lines_json" --argjson native_cases "$native_cases" '{
  format:"tondo-stdlib-sync-conformance-evidence/1",
  task:"STD-SYNC-CONF-001",
  status:"passed",
  source_revision:$revision,
  contract_sha256:("sha256:" + $contract_sha256),
  vm:{fixture:"tests/runtime/m11-std-sync-conformance-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:$vm_lines,status:"passed",log_sha256:("sha256:" + $vm_sha256)},
  native:{probe:"crates/tondo-native-runtime/examples/sync_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-sync-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
  static_capability:{fixture:"tests/compile-fail/m11-std-sync-conf-missing-threads.to",codes:["E1008"],driver_test:"passed",status:"passed"},
  collection_dependency:{task:"STD-SYNC-COLLECTION-CONF-001",status:"passed",report:"target/reliability/evidence/stdlib-sync-collection-conformance.json",report_sha256:("sha256:" + $collection_sha256)},
  comparison:{same_case_ids:true,same_corpus:true,vm_lines:true,native_observables:true,memory_orders:true,compare_exchange:true,parking_timeout:true,parking_wake_one:true,parking_wake_all:true,cleanup_stale_handles:true,once_publication_bridge:true,barrier_epoch_bridge:true,thread_lifecycle:true,collection_dependency:true,static_threads_rejection:true,cooperative_non_blocking:true,zero_live_objects:true},
  physical_paths:[],
  timestamps:false,
  addresses:[],
  divergences:[]
}' >"$evidence_dir/stdlib-sync-conformance.json"

echo "std.sync conformance: OK (8 shared VM/native-bridge cases; collection dependency; report: $evidence_dir/stdlib-sync-conformance.json)"
