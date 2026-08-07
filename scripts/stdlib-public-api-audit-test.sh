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

echo "stdlib public API audit tests: OK"
