#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_LOG_PERF_CONTRACT:-testing/stdlib-log-performance.json}"
parent="${TONDO_STDLIB_LOG_CONTRACT:-testing/stdlib-log.json}"
env TONDO_STDLIB_LOG_CONTRACT="$parent" bash scripts/stdlib-log-test-check.sh >/dev/null
python3 -B scripts/stdlib_log_performance.py check-contract --contract "$contract"
jq -e --slurpfile child "$contract" '
  .model.status == "verified"
  and .measurement.status == $child[0].status
  and .measurement.target == $child[0].target
  and .measurement.backend == $child[0].backend
  and .measurement.profile == $child[0].profile
  and .measurement.samples_per_workload == $child[0].protocol.minimum_sample_count
' "$parent" >/dev/null
while IFS= read -r path; do test -s "$path"; done \
  < <(jq -r '.contract,.parent_contract,.probe.path,.oracle.sources[]' "$contract")
for path in scripts/stdlib-log-performance.sh scripts/stdlib-log-performance-check.sh scripts/stdlib-log-performance-test.sh; do
    test -x "$path"
done
grep -Fq 'mod performance;' crates/tondo-compiler/src/process_host/log.rs
grep -Fq 'stdlib-log-performance.md' docs/contracts/stdlib-log.md
grep -Fq 'stdlib-log-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md
echo 'std.log performance contract: OK (18 hosted scalar VM workloads; qualified test profile)'
