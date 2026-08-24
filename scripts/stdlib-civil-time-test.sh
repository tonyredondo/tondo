#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-civil-time-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.time civil tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.capabilities.optional = ["clock"]' testing/stdlib-civil-time.json > "$tmp_dir/missing-civil-clock.json"
expect_failure missing-civil-clock env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/missing-civil-clock.json" scripts/stdlib-civil-time-check.sh

jq '.capabilities.source_sets.anchor = ["civil-clock"]' testing/stdlib-civil-time.json > "$tmp_dir/missing-anchor-clock.json"
expect_failure missing-anchor-clock env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/missing-anchor-clock.json" scripts/stdlib-civil-time-check.sh

jq '.capabilities.compile_time_clock_query = true' testing/stdlib-civil-time.json > "$tmp_dir/compile-time-clock.json"
expect_failure compile-time-clock env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/compile-time-clock.json" scripts/stdlib-civil-time-check.sh

jq '.timezone.utc_fallback = true' testing/stdlib-civil-time.json > "$tmp_dir/utc-fallback.json"
expect_failure utc-fallback env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/utc-fallback.json" scripts/stdlib-civil-time-check.sh

jq '.timezone.gap_fold_policy_required = false' testing/stdlib-civil-time.json > "$tmp_dir/implicit-gap-fold.json"
expect_failure implicit-gap-fold env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/implicit-gap-fold.json" scripts/stdlib-civil-time-check.sh

jq '.parsing.leap_seconds = true' testing/stdlib-civil-time.json > "$tmp_dir/leap-seconds.json"
expect_failure leap-seconds env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/leap-seconds.json" scripts/stdlib-civil-time-check.sh

jq '.anchor.live_refresh = true' testing/stdlib-civil-time.json > "$tmp_dir/live-anchor.json"
expect_failure live-anchor env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/live-anchor.json" scripts/stdlib-civil-time-check.sh

jq '.data.bundle_identity = ["version"]' testing/stdlib-civil-time.json > "$tmp_dir/unhashed-bundle.json"
expect_failure unhashed-bundle env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/unhashed-bundle.json" scripts/stdlib-civil-time-check.sh

jq '.surface.selectable_operations = ["civil-now"]' testing/stdlib-civil-time.json > "$tmp_dir/selectable-clock.json"
expect_failure selectable-clock env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/selectable-clock.json" scripts/stdlib-civil-time-check.sh

jq '.surface.types[0] = "BrokenDate"' testing/stdlib-civil-time.json > "$tmp_dir/signature-drift.json"
expect_failure signature-drift env TONDO_STDLIB_CIVIL_TIME_CONTRACT="$tmp_dir/signature-drift.json" scripts/stdlib-civil-time-check.sh

for marker in \
    'Date.addMonths(self, months: Int, policy: MonthPolicy)' \
    'UtcDateTime.inZone(self, zone: TimeZone)' \
    'UtcOffset.fromSeconds(seconds: Int)' \
    'ZoneDatabase.hash(self)' \
    'TimeZone.resolve(self, local: DateTime, policy: ResolvePolicy)' \
    'CivilClock.now()' \
    'CivilAnchor.toInstant(self, utc: UtcDateTime)' \
    'calendario gregoriano proléptico' \
    'target-declared-immutable' \
    'Reject' \
    'Earlier' \
    'Later' \
    'ShiftForward' \
    'DomainMismatch'; do
    grep -Fq "$marker" docs/contracts/stdlib-civil-time.md \
        || { echo "std.time civil tests: missing marker $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-CIVIL-TIME-001"
  and .capabilities.required == []
  and .capabilities.optional == ["civil-clock", "clock"]
  and .capabilities.source_sets.civil_clock == ["civil-clock"]
  and .capabilities.source_sets.anchor == ["civil-clock", "clock"]
  and .data.bundle_identity == ["version", "sha256"]
  and ((.data.ambient_sources_forbidden | index("TZ")) != null)
  and .parsing.leap_seconds == false
  and .timezone.gap_fold_policy_required == true
  and .timezone.utc_fallback == false
  and .anchor.domain_check == "CivilError.DomainMismatch"
  and .anchor.live_refresh == false
  and .implementation.public_api_promoted == false
  and .promotion.next_blocks == ["STD-REGEX-001", "DIAG-RUNTIME-001"]
' testing/stdlib-civil-time.json >/dev/null

echo "std.time civil tests: OK (negative capabilities; parsing; versioned zones; gap/fold policies; anchor boundary)"
