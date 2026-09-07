#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_YAML_CONFORMANCE_CONTRACT:-$root/testing/stdlib-yaml-conformance.json}"

die() {
    echo "std.yaml conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-yaml-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.yaml"
  and .task == "STD-YAML-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-yaml.json"
  and .document == "docs/contracts/stdlib-yaml-conformance.md"
  and .vm.fixture == "tests/runtime/m11-std-yaml-conformance-001.to"
  and .vm.command == "cargo run -q -p tondo-cli --locked -- run tests/runtime/m11-std-yaml-conformance-001.to"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 7)
  and .vm.expected_stdout[0] == "typed-dynamic:3:Tondo:7:2"
  and .vm.expected_stdout[1] == "interoperability:3:16:fo:3"
  and .vm.expected_stdout[2] == "streaming:2:16:closed"
  and .vm.expected_stdout[3] == "errors-path:17:2:0"
  and .vm.expected_stdout[4] == "limits-lifecycle:19:0"
  and .vm.expected_stdout[5] == "route-boundary:scalar:simd-not-claimed:native-aot-not-claimed"
  and .vm.expected_stdout[6] == "yaml-conformance-ok"
  and .native.status == "verified-native-stdlib-process"
  and .native.probe == "crates/tondo-native-runtime/examples/yaml_conformance.rs"
  and .native.command == "cargo run -q -p tondo-native-runtime --example yaml_conformance --locked"
  and .native.target_policy == "host-target-only-until-native-aot-yaml-lowering"
  and .native.abi == "not-implemented"
  and .native.aot == "not-claimed"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.observable == "exact-ordered-VM-lines-plus-native-json-observations"
  and .rules.scalar == "native-probe-reuses-tondo-stdlib-yaml-without-a-second-wire-model"
  and .rules.simd == "not-measured-no-optimized-route"
  and .rules.native_aot == "not-claimed"
  and .rules.cleanup == "fresh-case-runtime-reset-and-zero-live-runtime-table-objects-before-return"
  and (.cases | length == 6)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and ([.cases[].vm_observable] == .vm.expected_stdout[0:6])
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .native_expected.status == "passed" and (.vm_observable | length > 0))
  and .cases[0].native_expected == {status:"passed",line:"typed-dynamic:3:Tondo:7:2",dynamic_keys:3,typed_length:2,cleanup:true}
  and .cases[1].native_expected == {status:"passed",line:"interoperability:3:16:fo:3",canonical_keys:3,binary:"fo",cleanup:true}
  and .cases[2].native_expected == {status:"passed",line:"streaming:2:16:closed",documents:2,bytes_events:16,chunk_events:16,terminal:true,cleanup:true}
  and .cases[3].native_expected == {status:"passed",line:"errors-path:17:2:0",kind:17,path:["items","0"],offset:0,line:1,column:1,cleanup:true}
  and .cases[4].native_expected == {status:"passed",line:"limits-lifecycle:19:0",kind:19,offset:0,closed:true,cleanup:true}
  and .cases[5].native_expected == {status:"passed",line:"route-boundary:scalar:simd-not-claimed:native-aot-not-claimed",scalar:"verified",simd:"not-measured-no-optimized-route",native_aot:"not-claimed"}
  and (.negative_cases | length == 14)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and .report == "target/reliability/evidence/stdlib-yaml-conformance.json"
  and .next_blocks == ["STD-TOML-IMPL-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-yaml.json \
    testing/stdlib-yaml-test.json \
    docs/contracts/stdlib-yaml.md \
    docs/contracts/stdlib-yaml-test.md \
    docs/contracts/stdlib-yaml-performance.md \
    docs/contracts/stdlib-yaml-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-yaml-conformance-001.to \
    tests/runtime/m11-std-yaml-conformance-001.stdout \
    tests/runtime/m11-std-yaml-conformance-001.exit \
    crates/tondo-stdlib/src/yaml.rs \
    crates/tondo-compiler/src/hir/check.rs \
    crates/tondo-compiler/src/hir/lower.rs \
    crates/tondo-compiler/src/process_host.rs \
    crates/tondo-native-runtime/examples/yaml_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in \
    scripts/stdlib-yaml-conformance-check.sh \
    scripts/stdlib-yaml-conformance-test.sh \
    scripts/stdlib-yaml-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for marker in \
    'YamlReaderFromBytes' \
    'YamlReaderFromReader' \
    'YamlWriterToWriter' \
    'std.yaml.YamlReader.fromBytes' \
    'std.yaml.YamlReader.fromReader' \
    'std.yaml.YamlReader.next' \
    'std.yaml.YamlWriter.finish'; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/hir/check.rs" "$root/crates/tondo-compiler/src/hir/lower.rs" "$root/crates/tondo-compiler/src/process_host.rs" \
        || die "YAML hosted bridge marker is missing: $marker"
done

for marker in \
    'parse_all_with_options' \
    'decode_static' \
    'YamlReader::from_reader' \
    'YamlErrorKind::InvalidBinary' \
    'YamlErrorKind::Closed'; do
    grep -Fq "$marker" "$root/crates/tondo-native-runtime/examples/yaml_conformance.rs" \
        || die "native YAML corpus marker is missing: $marker"
done

for marker in \
    'std.yaml conformance' \
    'same corpus' \
    'fragmentos de un byte' \
    'zero live runtime-table objects' \
    'native_aot: not-claimed' \
    'not-measured-no-optimized-route' \
    'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-yaml-conformance.md" \
        || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-YAML-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-yaml-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-yaml-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-stdlib-process"
  and .conformance.cases == 6
  and .conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-TOML-IMPL-001"]
' "$root/testing/stdlib-yaml.json" >/dev/null || die "YAML owner registry does not expose conformance promotion"

jq -e '.promotion.next_blocks == ["STD-TOML-IMPL-001"]' \
    "$root/testing/stdlib-yaml-test.json" >/dev/null \
    || die "YAML test registry has a stale conformance frontier"

grep -Fq 'stdlib-yaml-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link YAML conformance"
grep -Fq 'stdlib-yaml-conformance.md' "$root/docs/contracts/stdlib-yaml.md" \
    || die "YAML owner document does not link conformance"
grep -Fq 'STD-YAML-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record YAML conformance"

[[ "$(tr -d '\r\n' <tests/runtime/m11-std-yaml-conformance-001.exit)" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(wc -l <tests/runtime/m11-std-yaml-conformance-001.stdout)" == "7" ]] \
    || die "fixture stdout sidecar must contain seven lines"

echo "std.yaml conformance contract: OK (6 shared cases; typed/dynamic, streaming, errors, limits and explicit native/AOT boundary)"
