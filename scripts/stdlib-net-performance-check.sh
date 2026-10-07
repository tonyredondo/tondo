#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_PERF_CONTRACT:-testing/stdlib-net-performance.json}"
parent="${TONDO_STDLIB_NET_CONTRACT:-testing/stdlib-net.json}"
env TONDO_STDLIB_NET_CONTRACT="$parent" bash scripts/stdlib-net-test-check.sh >/dev/null
python3 -B scripts/stdlib_net_performance.py check-contract --contract "$contract"
jq -e --slurpfile child "$contract" '
  .measurement.status == $child[0].status
  and .measurement.target == $child[0].target
  and .measurement.backend == $child[0].backend
  and .measurement.profile == $child[0].profile
  and .measurement.samples_per_workload == $child[0].protocol.minimum_sample_count
' "$parent" >/dev/null
while IFS= read -r path; do test -f "$path"; done \
  < <(jq -r '.contract,.parent_contract,.probe.path,.oracle.sources[]' "$contract")
for path in scripts/stdlib-net-performance.sh scripts/stdlib-net-performance-check.sh scripts/stdlib-net-performance-test.sh; do
    test -x "$path"
done
grep -Fq 'mod performance;' crates/tondo-compiler/src/process_host/net.rs
grep -Fq 'stdlib-net-performance.md' docs/contracts/stdlib-net.md
grep -Fq 'stdlib-net-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md
echo 'std.net performance contract: OK (21 controlled hosted routes; no VM/native timing claim)'
