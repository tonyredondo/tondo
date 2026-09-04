#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENCODING_CONTRACT:-$root/testing/stdlib-encoding.json}"

die() {
    echo "std.encoding implementation: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf "\n") || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.encoding"
  and .task == "STD-ENCODING-001"
  and .status == "contract-locked"
  and .target == "tondo-vm-hosted-and-native"
  and .implementation.status == "verified-hosted-vm"
  and .implementation.public_api_promoted == false
  and .implementation.host == "verified-hosted-vm-scalar-bridge"
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 12)
  and (.implementation.tests | type == "array" and length == 14)
  and .implementation.fixture == {path:"tests/runtime/m11-std-encoding-impl-001.to",stdout:"Zm8=encoding-ok",exit:0,status:"passed"}
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-encoding-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == ["STD-ENCODING-TEST-001", "STD-ENCODING-PERF-001", "STD-ENCODING-CONF-001", "STD-ENCODING-DOC-001"]
  and .promotion.next_blocks == ["STD-ENCODING-TEST-001"]
' "$contract" >/dev/null || die "invalid machine-readable implementation state"

for path in \
    docs/contracts/stdlib-encoding.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-encoding-impl-001.to \
    tests/runtime/m11-std-encoding-impl-001.stdout \
    tests/runtime/m11-std-encoding-impl-001.exit; do
    [[ -f "$root/$path" ]] || die "missing implementation input: $path"
done

for script in \
    scripts/stdlib-encoding-implementation-check.sh \
    scripts/stdlib-encoding-implementation-test.sh \
    scripts/stdlib-encoding-implementation.sh; do
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
    "Base64Encoder" \
    "decode_base64_quantum" \
    "hexadecimal_streaming_and_chunk_limits_are_stable"; do
    grep -Fq "$marker" "$root/crates/tondo-stdlib/src/encoding.rs" \
        || die "scalar implementation anchor is missing: $marker"
done

for marker in \
    "lower_bootstrap_encoding_nominal_declarations" \
    "HirBootstrapHostFunction::EncodingBase64OptionsEncode" \
    "HirBootstrapHostFunction::EncodingBase64EncoderPush"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/hir/lower.rs" \
        || die "compiler lowering anchor is missing: $marker"
done

for marker in \
    "Self::EncodingBase64OptionsEncode" \
    "Self::EncodingHexDecoderFinish"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/hir.rs" \
        || die "compiler HIR anchor is missing: $marker"
done

for marker in \
    "EncodingBase64Encoder(encoding::Base64Encoder)" \
    "encoding_host_materialized_and_streaming_contract" \
    "std.encoding.Base64Options.encodeTo"; do
    grep -Fq "$marker" "$root/crates/tondo-compiler/src/process_host.rs" \
        || die "host bridge anchor is missing: $marker"
done

grep -Fq "RuntimeHostValueKind::EncodingBase64Encoder" "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "VM host-kind materialization is missing"
grep -Fq "fn encoding_host_kind" "$root/crates/tondo-vm/src/runtime/execute.rs" \
    || die "VM encoding host-kind guard is missing"
grep -Fq "std.encoding." "$root/crates/tondo-vm/src/bytecode/verify.rs" \
    || die "bytecode verifier does not recognize encoding host ABI"

fixture_root="${root}/tests/runtime/m11-std-encoding-impl-001"
[[ "$(tr -d "\r\n" <"$fixture_root.exit")" == "0" ]] \
    || die "fixture exit sidecar is not zero"
[[ "$(tr -d "\r\n" <"$fixture_root.stdout")" == "Zm8=encoding-ok" ]] \
    || die "fixture stdout sidecar is not Zm8=encoding-ok"

for marker in \
    "STD-ENCODING-IMPL-001" \
    "native_aot_lowering: not-claimed" \
    "ruta hosted y del oráculo scalar"; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-encoding.md" \
        || die "implementation contract document misses marker: $marker"
done
grep -Fq "[x] **STD-ENCODING-IMPL-001" "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the implementation leaf"
grep -Fq "STD-ENCODING-TEST-001" "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not expose the next encoding block"

echo "std.encoding implementation: OK (scalar kernel; hosted VM bridge; native AOT explicitly unclaimed)"
