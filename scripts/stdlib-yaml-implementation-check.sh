#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_YAML_CONTRACT:-$root/testing/stdlib-yaml.json}"

die() {
    echo "std.yaml implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf "\n") || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.yaml"
  and .task == "STD-YAML-001"
  and .status == "contract-locked"
  and .testing_contract == "testing/stdlib-yaml-test.json"
  and .testing_document == "docs/contracts/stdlib-yaml-test.md"
  and .target == "tondo-vm-hosted-and-native"
  and .implementation.status == "verified-hosted-vm"
  and .implementation.public_api_promoted == false
  and .implementation.host == "verified-hosted-vm-buffered-yaml-bridge"
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 17)
  and (.implementation.tests | type == "array" and length == 11)
  and .implementation.fixture == {path:"tests/runtime/m11-std-yaml-impl-001.to",stdout:"yaml-ok",exit:0,status:"passed"}
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-yaml-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == ["STD-YAML-CONF-001", "STD-YAML-DOC-001"]
  and .promotion.next_blocks == ["STD-YAML-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable implementation state"

for path in \
    docs/contracts/stdlib-yaml.md \
    docs/contracts/stdlib-yaml-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-yaml-impl-001.to \
    tests/runtime/m11-std-yaml-impl-001.stdout \
    tests/runtime/m11-std-yaml-impl-001.exit; do
    [[ -f "$root/$path" ]] || die "missing implementation input: $path"
done

for script in \
    scripts/stdlib-yaml-implementation-check.sh \
    scripts/stdlib-yaml-implementation-test.sh \
    scripts/stdlib-yaml-implementation.sh; do
    [[ -x "$root/$script" ]] || die "implementation runner is not executable: $script"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing implementation source: $path"
done < <(jq -r ".implementation.sources[]" "$contract")

while IFS= read -r test; do
    case "$test" in
        scripts/*)
            [[ -x "$root/$test" ]] || die "implementation test script is not executable: $test"
            ;;
        *::*)
            file="${test%%::*}"
            name="${test##*::}"
            [[ -f "$root/$file" ]] || die "missing test source: $file"
            grep -Fq "$name" "$root/$file" || die "missing test anchor: $test"
            ;;
        *)
            [[ -f "$root/$test" ]] || die "missing test source: $test"
            ;;
    esac
done < <(jq -r ".implementation.tests[]" "$contract")

for marker in \
    "pub enum YamlValue" \
    "parse_all_with_options" \
    "YamlReader" \
    "YamlWriter" \
    "core_block_flow_and_quotes_round_trip"; do
    grep -Fq "$marker" "$root/crates/tondo-stdlib/src/yaml.rs" \
        || die "scalar implementation anchor is missing: $marker"
done

for marker in \
    "lower_bootstrap_yaml_nominal_declarations" \
    "HirBootstrapHostFunction::YamlParse" \
    "HirBootstrapHostFunction::YamlReaderFromBytes"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/hir/lower.rs" \
        || die "compiler lowering anchor is missing: $marker"
done

for marker in \
    "Self::YamlEncode" \
    "Self::YamlWriterFinish"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/hir.rs" \
        || die "compiler HIR anchor is missing: $marker"
done

for marker in \
    "HostValue::YamlValue" \
    "yaml_public_host_surface_materializes_typed_values_and_streams" \
    "yaml_host_rejects_invalid_limits_and_forged_events_atomically"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/process_host.rs" \
        || die "host bridge anchor is missing: $marker"
done

grep -Fq "RuntimeHostValueKind::YamlValue" "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "VM host-kind materialization is missing"
grep -Fq "std.yaml.YamlReader." "$root/crates/tondo-vm/src/bytecode/verify.rs" \
    || die "bytecode verifier does not recognize YAML host ABI"

fixture_root="${root}/tests/runtime/m11-std-yaml-impl-001"
[[ "$(tr -d "\r\n" <"$fixture_root.exit")" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(tr -d "\r\n" <"$fixture_root.stdout")" == "yaml-ok" ]] \
    || die "fixture stdout sidecar is not yaml-ok"

for marker in \
    "STD-YAML-IMPL-001" \
    "native_aot_lowering: not-claimed" \
    "ruta hosted buffered y del oráculo scalar"; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-yaml.md" \
        || die "implementation contract document misses marker: $marker"
done
grep -Fq "[x] **STD-YAML-IMPL-001" "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the implementation leaf"
grep -Fq "STD-YAML-TEST-001" "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not expose the next YAML block"
grep -Fq "stdlib-yaml-test.json" "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the YAML testing contract"
grep -Fq "stdlib-yaml-test.md" "$root/docs/contracts/stdlib-yaml.md" \
    || die "YAML document does not link the testing contract"

echo "std.yaml implementation: OK (YAML scalar kernel; buffered hosted VM bridge; native AOT explicitly unclaimed)"
