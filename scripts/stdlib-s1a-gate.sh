#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
report="${CARGO_TARGET_DIR:-$root/target}/reliability/evidence/stdlib-s1a-readiness.json"
python3 -B scripts/stdlib_s1a_readiness.py --output "$report"
python3 -B scripts/stdlib_s1a_readiness_test.py
if jq -e '.status == "eligible-for-seal-validation"' "$report" >/dev/null; then
    scripts/stdlib-s1a-seal.sh
    scripts/stdlib-s1a-seal-test.sh
else
    # Prove the actual producer refuses open prerequisites, before it runs
    # expensive campaigns or attempts to construct a bundle.
    set +e
    scripts/stdlib-s1a-seal.sh > "$report.refusal.log" 2>&1
    result=$?
    set -e
    [[ "$result" == 3 ]] || { cat "$report.refusal.log" >&2; exit 1; }
    grep -Fq 'S1A seal blocked by open prerequisites' "$report.refusal.log"
    echo "S1A remains pending; refusal verified. See $report"
fi
