#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-core-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.core owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners = .owners[1:]' testing/stdlib-core.json > "$tmp_dir/missing-core-owner.json"
expect_failure missing-core-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-core-owner.json" scripts/stdlib-core-check.sh

jq '.test_matrix = []' testing/stdlib-core.json > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/missing-test-matrix.json" scripts/stdlib-core-check.sh

jq '.owners[0] = "std.invalid"' testing/stdlib-core.json > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner env TONDO_STDLIB_CORE_CONTRACT="$tmp_dir/invalid-owner.json" scripts/stdlib-core-check.sh

jq '.owners |= map(if .id == "std.core" then .cells.HOST.status = "verified" | .cells.HOST.reason = null else . end)' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/host-boundary.json"
expect_failure host-boundary env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/host-boundary.json" scripts/stdlib-owner-evidence-check.sh

jq '.owners[0].cells.TEST.refs[0] = "missing/std-core-test-reference"' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/missing-reference.json"
expect_failure missing-reference env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/missing-reference.json" scripts/stdlib-owner-evidence-check.sh

grep -Fq 'some(3)' tests/runtime/m11-std-core-002.to
grep -Fq 'ok(value)' tests/runtime/m11-std-core-002.to
grep -Fq 'mapErr' tests/runtime/m11-std-core-002.to
grep -Fq 'HirBootstrapHostFunction::CoreOptionMap' crates/tondo-compiler/src/hir/lower.rs
grep -Fq 'HirBootstrapHostFunction::CoreResultMapErr' crates/tondo-compiler/src/hir/lower.rs
grep -Fq 'fn lower_core_option_map' crates/tondo-compiler/src/mir/lower.rs
grep -Fq 'fn lower_core_result_map_err' crates/tondo-compiler/src/mir/lower.rs
grep -Fq 'BytecodeTag::OptionSome' crates/tondo-vm/src/runtime/execute.rs
grep -Fq 'BytecodeTag::ResultErr' crates/tondo-vm/src/runtime/execute.rs
grep -Fq '2 => BytecodeTypeKind::Option' fuzz/fuzz_targets/admission.rs
grep -Fq '3 => BytecodeTypeKind::Result' fuzz/fuzz_targets/admission.rs

jq -e '
  ([.rows[] | select(.owner == "std.core" and .status == "verified")] | length) == 9
  and ([.rows[] | select(.owner == "std.core" and .missing == [])] | length) == 9
' testing/stdlib-public-api.json >/dev/null

echo "std.core owner tests: OK"
