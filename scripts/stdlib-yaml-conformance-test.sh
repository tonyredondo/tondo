#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-yaml-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.yaml conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "open"' testing/stdlib-yaml-conformance.json >"$tmp_dir/open.json"
expect_failure open-status env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/open.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.vm.fixture = "tests/runtime/m11-std-yaml-conformance-missing.to"' \
    testing/stdlib-yaml-conformance.json >"$tmp_dir/missing-vm-fixture.json"
expect_failure missing-vm-fixture env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/missing-vm-fixture.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.native.probe = "crates/tondo-native-runtime/examples/missing_yaml_conformance.rs"' \
    testing/stdlib-yaml-conformance.json >"$tmp_dir/missing-native-probe.json"
expect_failure missing-native-probe env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/missing-native-probe.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.cases[0].id = .cases[1].id' testing/stdlib-yaml-conformance.json >"$tmp_dir/duplicate-id.json"
expect_failure duplicate-id env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/duplicate-id.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.rules.same_case_ids = false' testing/stdlib-yaml-conformance.json >"$tmp_dir/corpus-drift.json"
expect_failure corpus-drift env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/corpus-drift.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.cases[0] |= del(.native_expected)' testing/stdlib-yaml-conformance.json >"$tmp_dir/native-case-missing.json"
expect_failure native-case-missing env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/native-case-missing.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.cases[1].vm_observable = "interoperability:drift"' \
    testing/stdlib-yaml-conformance.json >"$tmp_dir/observable-drift.json"
expect_failure observable-drift env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/observable-drift.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.cases[3].native_expected.path = ["wrong", "0"]' \
    testing/stdlib-yaml-conformance.json >"$tmp_dir/wrong-error-path.json"
expect_failure wrong-error-path env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/wrong-error-path.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.cases[2].native_expected.bytes_events = 15' \
    testing/stdlib-yaml-conformance.json >"$tmp_dir/event-count-drift.json"
expect_failure event-count-drift env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/event-count-drift.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.rules.scalar = "independent-second-wire-model"' testing/stdlib-yaml-conformance.json >"$tmp_dir/scalar-drift.json"
expect_failure scalar-drift env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/scalar-drift.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.rules.simd = "verified"' testing/stdlib-yaml-conformance.json >"$tmp_dir/simd-claim.json"
expect_failure simd-claim env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/simd-claim.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.rules.native_aot = "verified"' testing/stdlib-yaml-conformance.json >"$tmp_dir/native-aot-claim.json"
expect_failure native-aot-claim env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/native-aot-claim.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.report = "../../tmp/yaml.json"' testing/stdlib-yaml-conformance.json >"$tmp_dir/report-path.json"
expect_failure report-path env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/report-path.json" \
    scripts/stdlib-yaml-conformance-check.sh

jq '.next_blocks = ["STD-YAML-CONF-001"]' testing/stdlib-yaml-conformance.json >"$tmp_dir/promotion.json"
expect_failure promotion env TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT="$tmp_dir/promotion.json" \
    scripts/stdlib-yaml-conformance-check.sh

echo "std.yaml conformance tests: OK (14 negative contract mutations rejected)"
