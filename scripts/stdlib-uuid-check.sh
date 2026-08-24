#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_UUID_CONTRACT:-$root/testing/stdlib-uuid.json}"

die() {
    echo "std.uuid contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.uuid"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-ID-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-uuid.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B7"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .dependencies == ["std.bytes", "std.time"]
  and .capabilities.required == []
  and .capabilities.optional == ["entropy", "civil-clock"]
  and .capabilities.source_sets.core == []
  and .capabilities.source_sets.v4 == ["entropy"]
  and .capabilities.source_sets.v7 == ["civil-clock", "entropy"]
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_lookup == false
  and .capabilities.compile_time_query == false
  and .capabilities.missing_capability == "static-E1008"
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and ((.capabilities.forbidden | index("environment")) != null)
  and ((.capabilities.forbidden | index("global-rng")) != null)
  and ((.capabilities.forbidden | index("collision-registry")) != null)
  and ((.capabilities.forbidden | index("hidden-retry")) != null)
  and .standard.spec == "RFC 9562"
  and .standard.rfc == "9562"
  and .standard.obsoletes == "RFC 4122"
  and .standard.width_bits == 128
  and .standard.width_bytes == 16
  and .standard.variant == "rfc9562-10"
  and .standard.byte_order == "network-big-endian"
  and .standard.nil == "00000000-0000-0000-0000-000000000000"
  and .standard.max == "ffffffff-ffff-ffff-ffff-ffffffffffff"
  and .standard.parse_external_versions == true
  and .standard.parse_external_variants == true
  and .versions.generated == ["v4", "v5", "v7"]
  and .versions.parsed == "all-128-bit-values"
  and .versions.v4.purpose == "random"
  and .versions.v4.capabilities == ["entropy"]
  and .versions.v4.random_bits == 122
  and .versions.v4.provider_bytes == 16
  and .versions.v4.retry == "none"
  and .versions.v5.purpose == "name-based"
  and .versions.v5.algorithm == "SHA-1"
  and .versions.v5.capabilities == []
  and .versions.v5.input == "namespace-bytes-concat-name-bytes"
  and .versions.v5.deterministic == true
  and .versions.v5.security == "not-a-secret-or-authenticator"
  and .versions.v7.purpose == "unix-millisecond-time-ordered-prefix"
  and .versions.v7.capabilities == ["civil-clock", "entropy"]
  and .versions.v7.timestamp_bits == 48
  and .versions.v7.timestamp_unit == "unix-milliseconds-utc-no-leap-seconds"
  and .versions.v7.random_bits == 74
  and .versions.v7.provider_bytes == 10
  and .versions.v7.strict_monotonicity == false
  and .versions.v7.retry == "none"
  and .versions.generation_excluded == ["v1", "v2", "v3", "v6", "v8"]
  and .text.canonical == "8-4-4-4-12-lowercase-hex"
  and .text.accepted_forms == ["dashed-36", "urn-uuid-prefix-plus-dashed-36"]
  and .text.hex_case == "accept-both-output-lowercase"
  and .text.urn_prefix == "case-insensitive-urn-uuid"
  and .text.max_bytes == 45
  and .text.whitespace == "reject"
  and .text.braces == "reject"
  and .text.compact_32_hex == "reject"
  and .text.other_uri_schemes == "reject"
  and .text.normalization == "none"
  and .text.locale == "forbidden"
  and .bytes.exact_length == 16
  and .bytes.order == "network-big-endian"
  and .bytes.from_bytes == "copy"
  and .bytes.to_bytes == "copy"
  and .bytes.native_endian == "never"
  and .bytes.com_guid_layout == "not-supported"
  and .variant.values == ["Rfc9562", "Ncs", "Microsoft", "Future"]
  and .variant.generated == "Rfc9562-only"
  and .variant.version_observation == "nibble-0-through-15-informational-for-non-rfc"
  and .api.module == "std.uuid"
  and ([.api.functions[]] | sort) == ["compare", "fromBytes", "isMax", "isNil", "max", "nil", "parse", "toBytes", "toString", "v4", "v5", "v7", "variant", "version"]
  and .api.annotations == []
  and .api.no_async_duplicate_api == true
  and .api.selectable_operations == []
  and .api.provider_injection == "capability-source-set"
  and .api.testing_provider == "sealed-std.testing-envelope"
  and ([.surface.types[]] | sort) == ["Uuid", "UuidError", "UuidErrorKind", "UuidVariant"]
  and (.surface.signatures | length) == 14
  and ([.surface.signatures[].id] | unique | length) == 14
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.id == "v4" and .effect == "entropy")] | length) == 1
  and ([.surface.signatures[] | select(.id == "v7" and .effect == "civil-clock-and-entropy")] | length) == 1
  and ([.surface.signatures[] | select(.effect == "suspends")] | length) == 0
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.selectable_operations == []
  and .surface.no_async_duplicate_api == true
  and .ownership.uuid_copyable == true
  and .ownership.uuid_sendable == true
  and .ownership.uuid_shareable == true
  and .ownership.uuid_hashable == true
  and .ownership.uuid_orderable == true
  and .ownership.contains_host_handle == false
  and .ownership.mutable_aliases == false
  and .ownership.from_bytes_copies == true
  and .ownership.to_bytes_copies == true
  and .ownership.provider_state_published == false
  and .ownership.global_registry == false
  and ([.limits[].id] | sort) == ["max_entropy_bytes", "max_name_bytes", "max_text_bytes", "uuid_bytes", "vm_heap"]
  and .errors.type == "UuidError"
  and .errors.location == "text-byte-offset-or-provider-boundary"
  and .errors.partial_success == false
  and (.errors.kinds | length) == 14
  and ((.errors.kinds | unique | length) == (.errors.kinds | length))
  and ((.errors.kinds | index("TimestampOutOfRange")) != null)
  and ((.errors.kinds | index("EntropyFailure")) != null)
  and ((.errors.kinds | index("ClockFailure")) != null)
  and .performance.scalar_oracle == true
  and .performance.fixed_width_state_machine == true
  and .performance.simd_allowed_after_equivalence == true
  and .performance.compare_allocation == "none"
  and .performance.generation_state == "no-global-lock-counter-or-registry"
  and .performance.claims_before_perf_gate == "forbidden"
  and ([.test_matrix[].id] | unique | length) == 10
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | length) == 8
  and ([.corpora[].id] | unique | length) == 8
  and all(.corpora[]; .required == true and (.focus | length) > 0)
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .promotion.next_blocks == ["STD-LOG-001", "DIAG-RUNTIME-001"]
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-UUID-IMPL-001", "STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
' "$contract" >/dev/null || die "invalid machine-readable std.uuid contract"

for path in \
    docs/contracts/stdlib-uuid.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-ID-001' \
    'RFC 9562' \
    'Uuid.v4' \
    'Uuid.v5' \
    'Uuid.v7' \
    'UuidVariant' \
    'Uuid.nil' \
    'Uuid.max' \
    'civil-clock' \
    'entropy' \
    'max_name_bytes' \
    'TimestampOutOfRange' \
    'valor inmutable de 128 bits' \
    'No existe un registro global' \
    'No se generan v1'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-uuid.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-uuid.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the UUID registry"

echo "std.uuid contract: OK (RFC 9562; v4/v5/v7; explicit capabilities; fixed-width value)"
