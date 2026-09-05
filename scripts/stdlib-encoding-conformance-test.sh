#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-encoding-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.encoding conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-encoding-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.vm.expected_stdout = .vm.expected_stdout[0:6]' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/missing-vm-case.json"
expect_failure missing-vm-case env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/missing-vm-case.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.cases = .cases[0:5]' testing/stdlib-encoding-conformance.json >"$tmp_dir/missing-native-case.json"
expect_failure missing-native-case env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/missing-native-case.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-encoding-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-encoding-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.cases[3].native_expected.base64_kind = 3' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/wrong-error-kind.json"
expect_failure wrong-error-kind env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/wrong-error-kind.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.cases[4].native_expected.closed_kind = 2' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/wrong-closed-kind.json"
expect_failure wrong-closed-kind env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/wrong-closed-kind.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.rules.scalar = "independent-second-implementation"' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/scalar-drift.json"
expect_failure scalar-drift env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/scalar-drift.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.rules.simd = "verified"' testing/stdlib-encoding-conformance.json >"$tmp_dir/simd-claim.json"
expect_failure simd-claim env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/simd-claim.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.rules.native_aot = "verified"' testing/stdlib-encoding-conformance.json >"$tmp_dir/native-aot-claim.json"
expect_failure native-aot-claim env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/native-aot-claim.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.native.target_policy = "all-targets"' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/target-policy.json"
expect_failure target-policy env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/target-policy.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.report = "../../tmp/encoding.json"' testing/stdlib-encoding-conformance.json >"$tmp_dir/report-path.json"
expect_failure report-path env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/report-path.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.next_blocks = ["STD-ENCODING-IMPL-001"]' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/promotion.json"
expect_failure promotion env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-encoding-conformance-check.sh

jq '.cases[5].native_expected.native_aot = "verified"' \
    testing/stdlib-encoding-conformance.json >"$tmp_dir/native-case-aot-claim.json"
expect_failure native-case-aot-claim env TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT="$tmp_dir/native-case-aot-claim.json" \
    scripts/stdlib-encoding-conformance-check.sh

echo "std.encoding conformance tests: OK (14 negative contract mutations rejected)"
