#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CIVIL_TIME_CONTRACT:-$root/testing/stdlib-civil-time.json}"

die() {
    echo "std.time civil contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.time.civil"
  and .parent_owner == "std.time"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CIVIL-TIME-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-civil-time.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B5"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .capabilities.required == []
  and .capabilities.optional == ["civil-clock", "clock"]
  and .capabilities.source_sets.core == "none"
  and .capabilities.source_sets.zones == "target-declared-timezone-bundle"
  and .capabilities.source_sets.civil_clock == ["civil-clock"]
  and .capabilities.source_sets.anchor == ["civil-clock", "clock"]
  and .capabilities.missing_civil_clock == "static-capability-error"
  and .capabilities.missing_clock_for_anchor == "static-capability-error"
  and .capabilities.compile_time_clock_query == false
  and .capabilities.import_effect == "none"
  and .capabilities.ambient_timezone_lookup == false
  and ((.capabilities.forbidden | unique | length) == (.capabilities.forbidden | length))
  and .data.calendar == "proleptic-gregorian"
  and .data.year_min == 1
  and .data.year_max == 9999
  and .data.seconds == "0..59-no-leap-seconds"
  and .data.offset_seconds == [-86399, 86399]
  and .data.zone_id_max_bytes == 255
  and .data.bundle == "immutable-target-input"
  and .data.bundle_identity == ["version", "sha256"]
  and .data.ambient_sources_forbidden == ["TZ", "locale", "LANG", "LC_*", "filesystem", "network", "environment", "operating-system-local-zone"]
  and .surface.types == [
    "Date", "Time", "DateTime", "UtcDateTime", "UtcOffset", "ZoneId", "ZoneDataVersion",
    "ZoneDatabase", "TimeZone", "ZonedDateTime", "CivilAnchor",
    "CivilError = { InvalidDate, InvalidTime, InvalidOffset, InvalidZoneId, ZoneUnavailable, ZoneDataUnavailable, NonexistentLocalTime, AmbiguousLocalTime, OutOfRange, DomainMismatch, Unavailable, ResourceLimit }",
    "MonthPolicy = { Reject, Clamp }",
    "ResolvePolicy = { Reject, Earlier, Later, ShiftForward }"
  ]
  and ([.surface.signatures[].id] | unique | length) == 57
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and ([.surface.signatures[] | select(.effect == "civil-clock") | .id] | sort) == ["civil-now"]
  and ([.surface.signatures[] | select(.effect == "civil-clock-and-clock") | .id] | sort) == ["civil-sample"]
  and ([.surface.signatures[] | select(.effect == "target-data") | .id] | sort) == ["zone-database"]
  and ([.surface.signatures[] | select(.effect == "pure")] | length) == 54
  and .surface.direct_call_waits == false
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "unchanged-by-contract"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == []
  and .value_model.host_handles == false
  and .value_model.mutable_aliases == false
  and .value_model.zoned_equality == "utc-local-zone-id-and-bundle-version"
  and .value_model.anchor_domain == "preserved-and-checked"
  and .parsing.date == "YYYY-MM-DD"
  and .parsing.time == "HH:MM:SS[.fraction-1-to-9-digits]"
  and .parsing.datetime == "YYYY-MM-DDTHH:MM:SS[.fraction-1-to-9-digits]"
  and .parsing.utc == "YYYY-MM-DDTHH:MM:SS[.fraction-1-to-9-digits]Z"
  and .parsing.zone == "canonical-IANA-ASCII-or-UTC"
  and .parsing.locale == "forbidden"
  and .parsing.permissive_fallback == false
  and .parsing.leap_seconds == false
  and .parsing.canonical_output == true
  and .arithmetic.duration_owner == "std.time.Duration"
  and .arithmetic.month_and_year_policy_required == true
  and .arithmetic.month_policy == ["Reject", "Clamp"]
  and .timezone.bundle == "target-declared-immutable"
  and .timezone.identity == ["version", "sha256"]
  and .timezone.lookup == "snapshot-only"
  and .timezone.offset_unit == "signed-seconds"
  and .timezone.offset_range == [-86399, 86399]
  and .timezone.gap_fold_policy_required == true
  and .timezone.resolve_policy == ["Reject", "Earlier", "Later", "ShiftForward"]
  and .timezone.gap_reject == "NonexistentLocalTime"
  and .timezone.fold_reject == "AmbiguousLocalTime"
  and .timezone.utc_fallback == false
  and .timezone.ambient_lookup == false
  and .anchor.requires == ["civil-clock", "clock"]
  and .anchor.domain_check == "CivilError.DomainMismatch"
  and .anchor.live_refresh == false
  and .anchor.epoch_conversion == false
  and .anchor.finite_horizon == true
  and ([.test_matrix[].id] | unique) == ["anchor", "capabilities", "cross-backend", "gaps-and-folds", "parsing", "pure-calendar", "timezone-bundle"]
  and all(.test_matrix[]; .required == true and (.observables | length) > 0)
  and ([.corpora[].id] | unique) == ["calendar-boundaries", "civil-anchor", "tzdb-transitions"]
  and all(.corpora[]; .source == "owner-generated" and .required == true and (.focus | length) > 0)
  and ([.promotion.gates[].id] == ["design", "implementation", "conformance", "performance", "promote"])
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-CIVIL-TIME-IMPL-001", "STD-CIVIL-TIME-HOST-001", "STD-CIVIL-TIME-TEST-001", "STD-CIVIL-TIME-PERF-001", "STD-CIVIL-TIME-CONF-001", "STD-CIVIL-TIME-DOC-001"]
  and .promotion.next_blocks == ["STD-ID-001", "STD-LOG-001", "DIAG-RUNTIME-001"]
' "$contract" >/dev/null || die "invalid machine-readable civil-time contract"

for path in \
    docs/contracts/stdlib-civil-time.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-CIVIL-TIME-001' \
    'pub type Date' \
    'pub type UtcDateTime' \
    'pub type UtcOffset' \
    'pub fn UtcOffset.fromSeconds(seconds: Int): UtcOffset ! CivilError' \
    'pub type ZoneDatabase' \
    'pub fn TimeZone.resolve(self, local: DateTime, policy: ResolvePolicy): ZonedDateTime ! CivilError' \
    'pub fn CivilClock.sample(): CivilAnchor ! CivilError' \
    'civil-clock' \
    'NonexistentLocalTime' \
    'AmbiguousLocalTime' \
    'target-declared-immutable' \
    'single-duration'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-civil-time.md" \
        || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-civil-time.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "main stdlib spec does not link the civil-time registry"

echo "std.time civil contract: OK (checked calendar; versioned zones; explicit gaps/folds; civil-clock anchor boundary)"
