#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"
contract="$root/testing/stdlib-async-group-conformance.json"
override="$(printenv TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT || true)"
if [[ -n "$override" ]]; then
    contract="$override"
fi
target_dir="$(printenv CARGO_TARGET_DIR || printf target)"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$(printenv TONDO_STDLIB_EVIDENCE_DIR || printf "$target_dir/reliability/evidence")"
logs_dir="$evidence_dir/stdlib-async-group-conformance-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.async.Group conformance: $*" >&2
    exit 1
}

TONDO_STDLIB_ASYNC_GROUP_CONFORMANCE_CONTRACT="$contract" \
    scripts/stdlib-async-group-conformance-check.sh

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
native_stdout_file="$logs_dir/native.stdout"
native_stderr_file="$logs_dir/native.stderr"
set +e
cargo run -q -p tondo-cli -- run tests/runtime/m11-std-async-group-001.to \
    >"$vm_stdout_file" 2>"$vm_stderr_file"
vm_exit=$?
set -e
vm_stdout="$(tr -d '\r' <"$vm_stdout_file" | sed '/^$/d' | tail -n 1)"
expected_exit="$(jq -r '.vm.expected_exit' "$contract")"
expected_stdout="$(jq -r '.vm.expected_stdout' "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr_file" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
[[ "$vm_stdout" == "$expected_stdout" ]] || die "hosted VM output differs: $vm_stdout"

set +e
cargo run -q -p tondo-native-runtime --example async_group_conformance --locked \
    >"$native_stdout_file" 2>"$native_stderr_file"
native_exit=$?
set -e
[[ "$native_exit" == 0 ]] || {
    cat "$native_stderr_file" >&2
    die "native Group probe failed"
}
native_cases="$(jq -s -c 'map(select(type == "object"))' "$native_stdout_file")"
expected_ids="$(jq -c '[.cases[].id]' "$contract")"
actual_ids="$(jq -c '[.[].id]' <<<"$native_cases")"
[[ "$actual_ids" == "$expected_ids" ]] || die "native case ids differ from shared corpus"
jq -e '
  length == 8
  and all(.[]; .status == "passed")
  and .[0] == {id:"group-all-order",status:"passed",all:"ok",remaining:0,outcomes:2,values:[1,2]}
  and .[1] == {id:"group-settle-mixed",status:"passed",settle:"ok",outcomes:2,values:[7,8],errors:[false,true]}
  and .[2] == {id:"group-all-error-priority",status:"passed",error_tag:3,error_payload:12,pending_drained:true,outcomes:1}
  and .[3] == {id:"group-next-order",status:"passed",indices:[1,0],values:[22,11],none:true}
  and .[4] == {id:"group-panic-drain",status:"passed",panic:true,cleanup:"exactly-once"}
  and .[5] == {id:"group-cancel-drain",status:"passed",cleanup:"exactly-once"}
  and .[6] == {id:"group-empty",status:"passed",all:true,settle:true,next_none:true}
  and .[7] == {id:"group-invalid-add",status:"passed",invalid_handle:true,joined_rejected:true}
' <<<"$native_cases" >/dev/null || die "native observations violate the Group oracle"

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d' ' -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-async-group-001.to | cut -d' ' -f1)"
probe_sha256="$(sha256sum crates/tondo-native-runtime/examples/async_group_conformance.rs | cut -d' ' -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d' ' -f1)"
native_sha256="$(sha256sum "$native_stdout_file" | cut -d' ' -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg probe_sha256 "$probe_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg native_sha256 "$native_sha256" \
    --argjson native_cases "$native_cases" \
    '{
      format:"tondo-stdlib-async-group-conformance-evidence/1",
      task:"STD-ASYNC-GROUP-CONF-001",
      status:"passed",
      source_revision:$revision,
      contract_sha256:("sha256:" + $contract_sha256),
      vm:{fixture:"tests/runtime/m11-std-async-group-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:"group-ok",status:"passed",log_sha256:("sha256:" + $vm_sha256)},
      native:{probe:"crates/tondo-native-runtime/examples/async_group_conformance.rs",probe_sha256:("sha256:" + $probe_sha256),status:"passed",target_policy:"host-target-only-until-native-aot-async-lowering",log_sha256:("sha256:" + $native_sha256),cases:$native_cases},
      comparison:{same_case_ids:true,same_corpus:true,ordering:true,error_priority:true,cancellation_drain:true,panic_drain:true,cleanup_exactly_once:true,static_rejection:true},
      physical_paths:[],
      timestamps:false,
      addresses:[],
      divergences:[]
    }' >"$evidence_dir/stdlib-async-group-conformance.json"

echo "std.async.Group conformance: OK (8 shared VM/native cases; report: $evidence_dir/stdlib-async-group-conformance.json)"
