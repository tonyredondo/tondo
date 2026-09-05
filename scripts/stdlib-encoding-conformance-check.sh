#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_ENCODING_CONFORMANCE_CONTRACT:-$root/testing/stdlib-encoding-conformance.json}"

die() {
    echo "std.encoding conformance: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing conformance contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-encoding-conformance/1"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .owner == "std.encoding"
  and .task == "STD-ENCODING-CONF-001"
  and .status == "verified"
  and .contract == "testing/stdlib-encoding.json"
  and .document == "docs/contracts/stdlib-encoding-conformance.md"
  and .vm.expected_exit == 0
  and (.vm.expected_stdout | length == 7)
  and .vm.expected_stdout[0] == "base64-interoperability:Zm8=:Zm8:fo"
  and .vm.expected_stdout[1] == "hex-policy:666f:666F:fo"
  and .vm.expected_stdout[2] == "streaming-invariance:Zm8:666F"
  and .vm.expected_stdout[3] == "strict-errors:4:0:4:2"
  and .vm.expected_stdout[4] == "limits-and-lifecycle:5:7:empty"
  and .vm.expected_stdout[5] == "route-boundary:scalar:simd-not-claimed"
  and .vm.expected_stdout[6] == "encoding-conformance-ok"
  and .native.status == "verified-native-runtime-abi"
  and .native.target_policy == "host-target-only-until-native-aot-encoding-lowering"
  and .rules.same_corpus == true
  and .rules.same_case_ids == true
  and .rules.fresh_process_per_probe == true
  and .rules.observable == "exact-ordered-lines-plus-native-result-tags"
  and .rules.interoperability == "materialized-Base64-and-hex-vectors-round-trip-across-the-shared-corpus"
  and .rules.streaming == "one-byte-fragments-and-quantum-boundaries-match-materialized-output"
  and .rules.errors == "strict-wire-errors-preserve-kind-and-observed-byte-offset"
  and .rules.limits == "input-output-limits-fail-before-publication-and-close-the-stream"
  and .rules.scalar == "native-private-ABI-reuses-the-stdlib-scalar-oracle-without-a-second-wire-model"
  and .rules.simd == "not-measured-no-optimized-route"
  and .rules.native_aot == "not-claimed"
  and .rules.cleanup == "fresh-case-reset-and-zero-live-objects-before-return"
  and (.cases | length == 6)
  and (([.cases[].id] | unique | length) == (.cases | length))
  and all(.cases[].id; test("^[a-z0-9-]+$"))
  and all(.cases[]; .native_expected.status == "passed" and (.vm_observable | length > 0))
  and .cases[0].native_expected == {status:"passed",standard:"Zm8=",url:"Zm8",round_trip:"fo",cleanup:true}
  and .cases[1].native_expected == {status:"passed",lower:"666f",upper:"666F",round_trip:"fo",cleanup:true}
  and .cases[2].native_expected == {status:"passed",base64:"Zm8",hex:"666F",terminal:true,cleanup:true}
  and .cases[3].native_expected == {status:"passed",base64_kind:4,base64_offset:0,hex_kind:4,hex_offset:2,cleanup:true}
  and .cases[4].native_expected == {status:"passed",limit_kind:5,limit_offset:0,closed_kind:7,zero_empty:true,cleanup:true}
  and .cases[5].native_expected == {status:"passed",scalar:"verified",simd:"not-measured-no-optimized-route",native_aot:"not-claimed"}
  and (.negative_cases | length == 14)
  and (([.negative_cases[]] | unique | length) == (.negative_cases | length))
  and .report == "target/reliability/evidence/stdlib-encoding-conformance.json"
  and .next_blocks == ["STD-ENCODING-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable conformance contract"

for path in \
    testing/stdlib-encoding.json \
    testing/stdlib-encoding-test.json \
    docs/contracts/stdlib-encoding.md \
    docs/contracts/stdlib-encoding-test.md \
    docs/contracts/stdlib-encoding-performance.md \
    docs/contracts/stdlib-encoding-conformance.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-encoding-conformance-001.to \
    tests/runtime/m11-std-encoding-conformance-001.stdout \
    tests/runtime/m11-std-encoding-conformance-001.exit \
    crates/tondo-stdlib/src/encoding.rs \
    crates/tondo-native-runtime/src/lib.rs \
    crates/tondo-native-runtime/examples/encoding_conformance.rs; do
    [[ -f "$root/$path" ]] || die "missing conformance input: $path"
done

for path in \
    scripts/stdlib-encoding-conformance-check.sh \
    scripts/stdlib-encoding-conformance-test.sh \
    scripts/stdlib-encoding-conformance.sh; do
    [[ -x "$root/$path" ]] || die "script is not executable: $path"
done

for symbol in \
    tondo_rt_buffer_from_bytes \
    tondo_rt_encoding_materialize \
    tondo_rt_encoding_stream_new \
    tondo_rt_encoding_push \
    tondo_rt_encoding_finish \
    tondo_rt_encoding_error_kind \
    tondo_rt_encoding_error_offset \
    NativeEncodingStream \
    native_encoding_error_kind; do
    grep -Fq "$symbol" "$root/crates/tondo-native-runtime/src/lib.rs" \
        || die "native conformance symbol is missing: $symbol"
done

for marker in \
    base64_interoperability \
    hex_policy \
    streaming_invariance \
    strict_errors \
    limits_and_lifecycle \
    route_boundary; do
    grep -Fq "$marker" "$root/crates/tondo-native-runtime/examples/encoding_conformance.rs" \
        || die "native corpus marker is missing: $marker"
done

for marker in \
    'std.encoding conformance' \
    'same case IDs' \
    'one-byte streaming' \
    'zero live native handles' \
    'native_aot_lowering: not-claimed' \
    'not-measured-no-optimized-route' \
    'physical paths'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-encoding-conformance.md" \
        || die "conformance document misses marker: $marker"
done

jq -e '
  .conformance.task == "STD-ENCODING-CONF-001"
  and .conformance.status == "verified"
  and .conformance.contract == "testing/stdlib-encoding-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-encoding-conformance.md"
  and .conformance.target == "tondo-vm-hosted-and-native-runtime-abi"
  and .conformance.cases == 6
  and .conformance.native_aot == "not-claimed"
  and .promotion.next_blocks == ["STD-ENCODING-DOC-001"]
' "$root/testing/stdlib-encoding.json" >/dev/null || die "owner registry does not expose conformance promotion"

jq -e '.promotion.next_blocks == ["STD-ENCODING-DOC-001"]' \
    "$root/testing/stdlib-encoding-test.json" >/dev/null \
    || die "test registry has a stale conformance frontier"

grep -Fq 'stdlib-encoding-conformance.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link encoding conformance"
grep -Fq 'stdlib-encoding-conformance.md' "$root/docs/contracts/stdlib-encoding.md" \
    || die "encoding document does not link conformance"
grep -Fq 'STD-ENCODING-CONF-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record encoding conformance"

[[ "$(tr -d '\r\n' <tests/runtime/m11-std-encoding-conformance-001.exit)" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(wc -l <tests/runtime/m11-std-encoding-conformance-001.stdout)" == "7" ]] \
    || die "fixture stdout sidecar must contain seven lines"

echo "std.encoding conformance contract: OK (6 shared cases; streaming, exact errors and explicit SIMD/AOT boundary)"
