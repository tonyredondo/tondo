#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-conformance.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
source_contract="testing/stdlib-toml-conformance.json"

scripts/stdlib-toml-conformance-check.sh >/dev/null

expect_failure() {
    local name="$1"
    local mutation="$2"
    local mutated="$tmp_dir/$name.json"
    jq "$mutation" "$source_contract" >"$mutated"
    if TONDO_STDLIB_TOML_CONFORMANCE_CONTRACT="$mutated" \
        scripts/stdlib-toml-conformance-check.sh >/dev/null 2>&1; then
        echo "std.toml conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

expect_failure open-status '.status = "open"'
expect_failure missing-fixture '.fixtures.files[0] = "missing.toml"'
expect_failure missing-vm-probe '.vm.probe = "crates/missing.rs"'
expect_failure missing-native-probe '.native.probe = "crates/missing.rs"'
expect_failure duplicate-id '.cases[1].id = .cases[0].id'
expect_failure corpus-drift '.rules.same_fixture_bytes = false'
expect_failure native-case-missing 'del(.cases[0].native_expected)'
expect_failure observable-drift '.cases[0].vm_observable = "wrong"'
expect_failure error-kind-drift '.cases[3].native_expected.kind = "InvalidKey"'
expect_failure error-path-drift '.cases[3].native_expected.path = ["wrong"]'
expect_failure error-span-drift '.cases[3].native_expected.offset = 0'
expect_failure event-count-drift '.cases[2].native_expected.bytes_events = 11'
expect_failure public-api-claim '.vm.public_compiler_api = "verified"'
expect_failure native-abi-claim '.native.abi = "verified"'
expect_failure native-aot-claim '.native.aot = "verified"'
expect_failure simd-claim '.rules.simd = "verified"'
expect_failure cleanup-drift '.cases[0].native_expected.cleanup = false'
expect_failure report-path '.report = "../../tmp/toml.json"'
expect_failure wrong-next-block '.next_blocks = ["STD-TOML-CONF-001"]'

echo "std.toml conformance tests: OK (19 negative contract mutations rejected)"
