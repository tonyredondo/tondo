#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target_dir="${CARGO_TARGET_DIR:-target-fast}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
export CARGO_TARGET_DIR="$target_dir"
export TMPDIR="${TONDO_NATIVE_TMPDIR:-$root/.tmp}"
mkdir -p "$TMPDIR"

scripts/native-diagnostics-check.sh
scripts/native-diagnostics-test.sh

# The hosted detector tests are the oracle fixtures.  Fuzzing is intentionally
# skipped here: the native parity lane must remain a bounded, reproducible
# executable check; the opt-in diagnostic CI workflow owns fuzz campaigns.
TONDO_DIAGNOSTIC_FUZZ_MODE=skip scripts/diagnostic-ci.sh --profile all
scripts/native-evaluation-runner.sh

report="$target_dir/reliability/evidence/native-evaluation-runner.json"
TONDO_NATIVE_DIAGNOSTICS_REPORT="$report" scripts/native-diagnostics-check.sh
echo "native diagnostics: PASS (hosted oracle and Cranelift/LLVM parity; report: ${report#"$root"/})"
