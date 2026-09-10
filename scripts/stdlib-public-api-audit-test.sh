#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-public-api-test.XXXXXX")"
# The host runner owns temporary-directory cleanup; this fixture is deliberately
# non-destructive when run from a shared test process.

# A documentation path must never become a public runtime case.  The strict
# checker is intentionally run against a temporary configuration so this
# proof never mutates the checked-in, open-gap report.
jq '
  .owners |= map(if .id == "std.core" then
    .case.path = "docs/contracts/stdlib-core.md"
    | .case.kind = "runtime"
  else . end)
' testing/stdlib-public-api-config.json > "$tmp/invalid.json"

set +e
TONDO_PUBLIC_API_CONFIG="$tmp/invalid.json" scripts/stdlib-public-api-audit.sh --strict >/dev/null 2>&1
strict_rc=$?
set -e
if [[ "$strict_rc" -eq 0 ]]; then
    echo "stdlib public API audit negative fixture unexpectedly passed" >&2
    exit 1
fi

# Assert the specific rejection, even while unrelated owners have open gaps.
TONDO_PUBLIC_API_CONFIG="$tmp/invalid.json" TONDO_PUBLIC_API_MATRIX="$tmp/invalid-report.json" \
    scripts/stdlib-public-api-audit.sh --write >/dev/null
jq -e 'first(.owners[] | select(.id == "std.core")) |
    .status == "open-gaps" and (.owner_missing | index("documentation-is-not-public-case") != null)' \
    "$tmp/invalid-report.json" >/dev/null

# A build-only label must not turn an empty extracted surface into coverage.
jq '.owners |= map(if .id == "std.core" then
    .include = ["nonexistentCallable"] |
    .runtime = {kind:"not-applicable",paths:[],reason:"negative fixture"} |
    .case = {path:"crates/tondo-compiler/src/driver.rs",kind:"build-only",canonical_calls:[]}
    else . end)' testing/stdlib-public-api-config.json > "$tmp/empty.json"
TONDO_PUBLIC_API_CONFIG="$tmp/empty.json" TONDO_PUBLIC_API_MATRIX="$tmp/empty-report.json" \
    scripts/stdlib-public-api-audit.sh --write >/dev/null
jq -e 'first(.owners[] | select(.id == "std.core")) |
    .signature_count == 0 and .status == "open-gaps" and
    (.owner_missing | index("no-callable-signatures-indexed") != null)' \
    "$tmp/empty-report.json" >/dev/null

# A real indexed runtime owner remains verified in the unmodified configuration.
TONDO_PUBLIC_API_MATRIX="$tmp/current-report.json" scripts/stdlib-public-api-audit.sh --write >/dev/null
jq -e 'first(.owners[] | select(.id == "std.core")) |
    .signature_count > 0 and .status == "verified"' "$tmp/current-report.json" >/dev/null

# Bodyless trait operations are public callables, with their declaring generic
# trait retained. They must not collapse into an empty build-only owner.
jq -e '
  [.rows[] | select(.owner == "std.serialization")] as $rows
  | ($rows | length) == 26
    and ([$rows[].declaring_trait] | unique | length) == 4
    and all($rows[]; .signature | startswith("fn "))
    and any($rows[]; .symbol == "std.serialization.Encoder.uint")
    and any($rows[]; .symbol == "std.serialization.Decode.decode")
    and all($rows[]; .status == "verified" and .evidence.public_case.kind == "runtime")
' "$tmp/current-report.json" >/dev/null

echo "stdlib public API audit tests: OK"
