#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_TOML_CONFORMANCE_CONTRACT:-$root/testing/stdlib-toml-conformance.json}"

die() {
    echo "std.toml conformance contract: $*" >&2
    exit 1
}

jq -e '
    .format == "tondo-stdlib-toml-conformance/1" and
    .edition == "0.1" and .owner == "std.toml" and
    .task == "STD-TOML-CONF-001" and
    .status == "verified-hosted-vm-adapter-and-native-stdlib-process" and
    .contract == "testing/stdlib-toml.json" and
    .document == "docs/contracts/stdlib-toml-conformance.md" and
    .fixtures.directory == "testing/stdlib-toml-conformance-fixtures" and
    .fixtures.files == ["typed-dynamic.toml","typed.toml","interoperability.toml","streaming.toml","duplicate.toml","limit.toml"] and
    .vm.probe == "crates/tondo-reliability/examples/toml_conformance_vm.rs" and
    .vm.command == "cargo run -q -p tondo-reliability --example toml_conformance_vm --locked" and
    .vm.expected_exit == 0 and
    .vm.boundary == "verified-bytecode-test-only-host-callable" and
    .vm.public_compiler_api == "not-implemented" and
    .native.probe == "crates/tondo-native-runtime/examples/toml_conformance.rs" and
    .native.command == "cargo run -q -p tondo-native-runtime --example toml_conformance --locked" and
    .native.status == "verified-native-stdlib-process" and
    .native.abi == "not-implemented" and .native.aot == "not-claimed" and
    .rules.same_fixture_bytes == true and .rules.same_case_ids == true and
    .rules.fresh_process_per_probe == true and
    .rules.simd == "not-measured-no-optimized-route" and
    .rules.native_aot == "not-claimed" and
    (.cases | length) == 6 and
    .report == "target/reliability/evidence/stdlib-toml-conformance.json"
' "$contract" >/dev/null || die "schema or promotion boundary differs"

actual_ids="$(jq -c '[.cases[].id]' "$contract")"
[[ "$actual_ids" == '["typed-dynamic","interoperability","streaming","errors-path","limits-lifecycle","route-boundary"]' ]] \
    || die "case IDs or order differ"
jq -e '
    [.cases[].vm_observable] + ["toml-conformance-ok"] == .vm.expected_stdout and
    ([.cases[] | select(.native_expected.status != "passed" or .native_expected.line != .vm_observable or .native_expected.cleanup != true)] | length) == 0 and
    ([.cases[] | .native_expected | has("id")] | any) == false and
    .cases[0].fixtures == ["typed-dynamic.toml","typed.toml"] and
    .cases[1].fixtures == ["interoperability.toml"] and
    .cases[2].fixtures == ["streaming.toml"] and
    .cases[3].fixtures == ["duplicate.toml"] and
    .cases[4].fixtures == ["limit.toml"] and
    .cases[5].fixtures == [] and
    .cases[2].native_expected.bytes_events == 12 and
    .cases[2].native_expected.chunk_events == 12 and
    .cases[3].native_expected.kind == "DuplicateKey" and
    .cases[3].native_expected.path == ["first"] and
    .cases[3].native_expected.offset == 10 and
    .cases[3].native_expected.start_line == 2 and
    .cases[3].native_expected.start_column == 1 and
    .cases[4].native_expected.kind == "ResourceLimit" and
    .cases[4].native_expected.partial_value == false and
    .next_blocks == ["STD-TOML-DOC-001"]
' "$contract" >/dev/null || die "corpus, observables or exact errors differ"

for file in "$(jq -r '.vm.probe' "$contract")" "$(jq -r '.native.probe' "$contract")"; do
    [[ -f "$file" ]] || die "missing probe $file"
done
[[ -f "$(jq -r '.document' "$contract")" ]] || die "missing conformance document"
for fixture in $(jq -r '.fixtures.files[]' "$contract"); do
    path="testing/stdlib-toml-conformance-fixtures/$fixture"
    [[ -f "$path" ]] || die "missing shared fixture $path"
    grep -Fq "$fixture" crates/tondo-reliability/examples/toml_conformance_vm.rs \
        || die "VM probe does not include $fixture"
    grep -Fq "$fixture" crates/tondo-native-runtime/examples/toml_conformance.rs \
        || die "native probe does not include $fixture"
done

echo "std.toml conformance contract: OK (6 shared cases; private VM host adapter and native stdlib process)"
