#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_ENCODING_CONTRACT:-$root/testing/stdlib-encoding.json}"

die() {
    echo "std.encoding contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.encoding"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-ENCODING-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-encoding.md"
  and .testing_contract == "testing/stdlib-encoding-test.json"
  and .testing_document == "docs/contracts/stdlib-encoding-test.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B6"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.bytes", "std.io"]
  and .capabilities.required == []
  and .capabilities.optional == []
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("ambient-host")) != null)
  and ((.capabilities.forbidden | index("runtime-value-reflection")) != null)
  and .wire.base64.standard_alphabet == "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
  and .wire.base64.url_safe_alphabet == "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
  and .wire.base64.padding == ["Required", "Omitted"]
  and .wire.base64.whitespace == "reject"
  and .wire.base64.line_wrapping == "forbidden"
  and .wire.base64.mixed_alphabet == "reject"
  and .wire.base64.non_zero_pad_bits == "reject"
  and .wire.hex.digits == "ASCII-0-9-a-f-A-F"
  and .wire.hex.odd_length == "reject"
  and .surface.types == [
    "Base64Alphabet", "Base64Padding", "HexCase", "EncodingErrorKind",
    "EncodingError", "EncodingLimits", "Base64Options", "HexOptions",
    "Base64Encoder", "Base64Decoder", "HexEncoder", "HexDecoder"
  ]
  and (.surface.signatures | length) == 30
  and ([.surface.signatures[].id] | unique | length) == 30
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "suspends") | .id] | sort) == ["base64-decode-from", "base64-encode-to", "hex-decode-from", "hex-encode-to"]
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .policies.base64.standard == {"alphabet":"Standard","padding":"Required"}
  and .policies.base64.url_safe == {"alphabet":"UrlSafe","padding":"Required"}
  and .policies.base64.url_safe_unpadded == {"alphabet":"UrlSafe","padding":"Omitted"}
  and .policies.base64.decode == "strict-policy-alphabet-padding-and-zero-pad-bits"
  and .policies.base64.canonical_output == true
  and .policies.hex.lower == {"output":"Lower","input":"Lower"}
  and .policies.hex.upper == {"output":"Upper","input":"Upper"}
  and .policies.hex.any_case == {"output":"Lower","input":"Lower-or-Upper"}
  and .policies.hex.canonical_output == true
  and .policies.no_ambient_defaults == true
  and .policies.no_permissive_decode == true
  and .streaming.materialized_is_collector == true
  and .streaming.chunk_boundary_invariant == true
  and .streaming.reader_input == "std.io.Reader-until-EOF"
  and .streaming.writer_output == "std.io.Writer-via-writeAll"
  and .streaming.base64_encoder_carry_bytes == 2
  and .streaming.base64_decoder_pending_chars == 3
  and .streaming.hex_decoder_pending_nibbles == 1
  and .streaming.empty_chunk == "no-state-change"
  and .streaming.finish_required == true
  and .streaming.error_state == "terminal"
  and .streaming.post_finish == "EncodingError.Closed"
  and .streaming.resource_limit_push == "atomic-no-state-change"
  and .streaming.partial_tondo_result == "never-published"
  and .ownership.bytes_owner == "std.bytes.Bytes"
  and .ownership.policies_copy == true
  and .ownership.stream_handles_affine == true
  and .ownership.stream_handles_copy == false
  and .ownership.stream_handles_share == false
  and .ownership.stream_handles_send == true
  and .ownership.writer_borrow == "ends-at-operation-return"
  and .ownership.input_alias == "never-retained"
  and ([.limits[].id] | unique) == ["base64_carry", "hex_pending_nibble", "max_input_bytes", "max_output_bytes", "vm_heap"]
  and .errors.type == "EncodingError"
  and .errors.location == "offset-is-observed-input-bytes-before-failure"
  and .errors.kinds == ["InvalidLimit", "InvalidCharacter", "InvalidLength", "InvalidPadding", "NonCanonical", "ResourceLimit", "Io", "Closed", "NoProgress"]
  and .errors.partial_success == false
  and .performance.scalar_oracle == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.dispatch == "target-declared-and-size-based"
  and .performance.claims_before_perf_gate == "forbidden"
  and .performance.task == "STD-ENCODING-PERF-001"
  and .performance.contract == "testing/stdlib-encoding-performance.json"
  and .performance.document == "docs/contracts/stdlib-encoding-performance.md"
  and .performance.status == "verified-hosted-vm-baseline"
  and .performance.target == "tondo-vm-hosted"
  and .performance.workloads == 16
  and .performance.samples_per_workload == 27
  and .performance.native_aot == "not-claimed"
  and .performance.dispatch_mode == "scalar-only"
  and .performance.code_size == "not-measured-by-hosted-report"
  and .performance.simd_crossover == "not-applicable-no-optimized-route"
  and ([.test_matrix[].id] | unique) == ["base64-canonical", "fragmentation", "hex-canonical", "lifecycle-and-errors", "limits-and-atomicity", "policy-catalog", "scalar-and-optimized-equivalence"]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["base64-invalid", "base64-rfc4648", "fragmentation-and-limits", "hex-vectors"]
  and ([.corpora[].id] | unique | length) == (.corpora | length)
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
 and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .implementation.status == "verified-hosted-vm"
 and .implementation.public_api_promoted == false
  and .implementation.host == "verified-hosted-vm-scalar-bridge"
  and .implementation.native_aot_lowering == "not-claimed"
  and (.implementation.sources | type == "array" and length == 12)
  and (.implementation.tests | type == "array" and length == 14)
  and .implementation.fixture == {path:"tests/runtime/m11-std-encoding-impl-001.to",stdout:"Zm8=encoding-ok",exit:0,status:"passed"}
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-encoding-implementation.json"
  and (.implementation.proof | type == "string" and length > 0)
  and .implementation.required_follow_ups == []
  and .documentation == {
    task: "STD-ENCODING-DOC-001",
    status: "verified",
    document: "docs/contracts/stdlib-encoding.md",
    fixture: "tests/runtime/m11-std-encoding-doc-001.to",
    command: "scripts/stdlib-encoding-doc-check.sh",
    expected_stdout: "Zm8=encoding-doc-ok",
    examples: [
      "policy-selection",
      "materialized-round-trip",
      "streaming-chunk-invariance",
      "strict-errors-and-limits",
      "costs-and-ownership",
      "writer-boundary"
    ],
    sections: [
      "surface",
      "policies",
      "errors",
      "costs",
      "ownership",
      "materialized-examples",
      "streaming-examples",
      "executable-verification"
    ]
  }
  and .promotion.next_blocks == ["STD-YAML-IMPL-001"]
' "$contract" >/dev/null || die "invalid machine-readable encoding contract"

for path in \
    docs/contracts/stdlib-encoding.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    tests/runtime/m11-std-encoding-doc-001.to \
    tests/runtime/m11-std-encoding-doc-001.stdout \
    tests/runtime/m11-std-encoding-doc-001.exit; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-ENCODING-001' \
    'pub enum Base64Alphabet' \
    'pub enum Base64Padding' \
    'pub enum HexCase' \
    'pub type EncodingError' \
    'pub type EncodingLimits' \
    'pub fn Base64Options.encode(self, input: Bytes): Bytes ! EncodingError' \
    'pub fn Base64Options.decodeFrom(self, var input: std.io.Reader): Bytes ! EncodingError suspends' \
    'pub fn HexOptions.anyCase(limits: EncodingLimits): HexOptions' \
    'pub fn Base64Encoder.finish(var self): Bytes ! EncodingError' \
    'pub fn HexDecoder.finish(var self): Bytes ! EncodingError' \
    'std.io.Reader' \
    'std.io.Writer' \
    'SIMD' \
    'NonCanonical' \
    'finish'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-encoding.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-encoding.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the encoding registry"
grep -Fq 'testing/stdlib-encoding-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the encoding testing registry"
grep -Fq 'stdlib-encoding-test.md' "$root/docs/contracts/stdlib-encoding.md" \
    || die "encoding document does not link the testing contract"
grep -Fq 'stdlib-encoding-doc-check.sh' "$root/docs/contracts/stdlib-encoding.md" \
    || die "encoding document does not link the documentation checker"
[[ -x "$root/scripts/stdlib-encoding-doc-check.sh" ]] \
    || die "encoding documentation checker is not executable"
[[ -x "$root/scripts/stdlib-encoding-doc-test.sh" ]] \
    || die "encoding documentation contract test is not executable"
scripts/stdlib-encoding-doc-check.sh >/dev/null

echo "std.encoding contract: OK (Base64/hex policies; strict canonicality; bounded streaming; scalar/SIMD boundary)"
