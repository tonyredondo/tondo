#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENCODING_CONTRACT:-$root/testing/stdlib-encoding.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-$target_dir/reliability/evidence}"
logs_dir="$evidence_dir/stdlib-encoding-implementation-logs"
mkdir -p "$evidence_dir" "$logs_dir"

die() {
    echo "std.encoding implementation: $*" >&2
    exit 1
}

TONDO_STDLIB_ENCODING_CONTRACT="$contract" scripts/stdlib-encoding-implementation-check.sh

vm_stdout_file="$logs_dir/vm.stdout"
vm_stderr_file="$logs_dir/vm.stderr"
set +e
CARGO_TARGET_DIR="$target_dir" cargo run -q -p tondo-cli --locked -- \
    run tests/runtime/m11-std-encoding-impl-001.to \
    >"$vm_stdout_file" 2>"$vm_stderr_file"
vm_exit=$?
set -e
expected_exit="$(jq -r ".implementation.fixture.exit" "$contract")"
expected_stdout="$(jq -r ".implementation.fixture.stdout" "$contract")"
[[ "$vm_exit" == "$expected_exit" ]] || {
    cat "$vm_stderr_file" >&2
    die "hosted VM exited $vm_exit, expected $expected_exit"
}
vm_stdout="$(tr -d "\r" <"$vm_stdout_file" | sed "/^$/d" | tail -n 1)"
[[ "$vm_stdout" == "$expected_stdout" ]] || die "hosted VM output differs: $vm_stdout"

stdlib_test_log="$logs_dir/stdlib-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib --locked \
    >"$stdlib_test_log" 2>&1 \
    || { cat "$stdlib_test_log" >&2; die "scalar stdlib tests failed"; }

compiler_test_log="$logs_dir/compiler-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-compiler --locked \
    encoding_host -- --nocapture \
    >"$compiler_test_log" 2>&1 \
    || { cat "$compiler_test_log" >&2; die "hosted encoding tests failed"; }

vm_test_log="$logs_dir/vm-tests.log"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-vm --locked \
    default_host_and_closed_runtime_helpers_have_explicit_boundaries -- --nocapture \
    >"$vm_test_log" 2>&1 \
    || { cat "$vm_test_log" >&2; die "VM host-kind tests failed"; }

source_revision="$(git rev-parse HEAD)"
contract_sha256="$(sha256sum "$contract" | cut -d" " -f1)"
fixture_sha256="$(sha256sum tests/runtime/m11-std-encoding-impl-001.to | cut -d" " -f1)"
vm_sha256="$(sha256sum "$vm_stdout_file" | cut -d" " -f1)"
stdlib_test_sha256="$(sha256sum "$stdlib_test_log" | cut -d" " -f1)"
compiler_test_sha256="$(sha256sum "$compiler_test_log" | cut -d" " -f1)"
vm_test_sha256="$(sha256sum "$vm_test_log" | cut -d" " -f1)"
jq -n \
    --arg revision "$source_revision" \
    --arg contract_sha256 "$contract_sha256" \
    --arg fixture_sha256 "$fixture_sha256" \
    --arg vm_sha256 "$vm_sha256" \
    --arg stdlib_test_sha256 "$stdlib_test_sha256" \
    --arg compiler_test_sha256 "$compiler_test_sha256" \
    --arg vm_test_sha256 "$vm_test_sha256" \
    '
      {
        format:"tondo-stdlib-encoding-implementation-evidence/1",
        task:"STD-ENCODING-IMPL-001",
        status:"passed",
        source_revision:$revision,
        contract_sha256:("sha256:" + $contract_sha256),
        vm:{fixture:"tests/runtime/m11-std-encoding-impl-001.to",fixture_sha256:("sha256:" + $fixture_sha256),exit:0,stdout:"Zm8=encoding-ok",status:"passed",log_sha256:("sha256:" + $vm_sha256)},
        hosted_tests:{filter:"encoding_host",status:"passed",log_sha256:("sha256:" + $compiler_test_sha256)},
        scalar_tests:{package:"tondo-stdlib",status:"passed",log_sha256:("sha256:" + $stdlib_test_sha256)},
        vm_tests:{filter:"default_host_and_closed_runtime_helpers_have_explicit_boundaries",status:"passed",log_sha256:("sha256:" + $vm_test_sha256)},
        public_boundary:{api_promoted:false,native_runtime:"not-claimed",native_aot_lowering:"not-claimed"},
        physical_paths:[],
        timestamps:false,
        addresses:[],
        divergences:[]
      }
    ' >"$evidence_dir/stdlib-encoding-implementation.json"

echo "std.encoding implementation: OK (hosted VM and scalar kernel; report: $evidence_dir/stdlib-encoding-implementation.json)"
