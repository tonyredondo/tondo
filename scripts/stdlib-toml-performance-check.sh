#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_TOML_PERF_CONTRACT:-$root/testing/stdlib-toml-performance.json}"

python3 -B scripts/stdlib_toml_performance.py check-contract --contract "$contract"

for path in \
    docs/contracts/stdlib-toml-performance.md \
    docs/contracts/stdlib-toml.md \
    docs/contracts/stdlib-toml-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-toml.json \
    testing/stdlib-toml-test.json \
    crates/tondo-reliability/src/toml_model.rs \
    crates/tondo-reliability/tests/toml_models.rs; do
    [[ -f "$path" ]] || { echo "std.toml performance: missing $path" >&2; exit 1; }
done
for path in \
    scripts/stdlib-toml-performance-check.sh \
    scripts/stdlib-toml-performance-test.sh \
    scripts/stdlib-toml-performance.sh; do
    [[ -x "$path" ]] || { echo "std.toml performance: not executable: $path" >&2; exit 1; }
done

jq -e '
  .performance.task == "STD-TOML-PERF-001"
  and .performance.contract == "testing/stdlib-toml-performance.json"
  and .performance.document == "docs/contracts/stdlib-toml-performance.md"
  and .performance.status == "verified-stdlib-kernel-baseline"
  and .performance.target == "x86_64-unknown-linux-gnu"
  and .performance.workloads == 13
  and .performance.samples_per_workload == 27
  and .performance.native_aot == "not-claimed"
  and .performance.hosted_vm == "not-claimed-no-toml-bridge"
  and .promotion.next_blocks == ["STD-TOML-DOC-001"]
  and .implementation.host == "not-claimed-until-compiler-toml-abi"
' testing/stdlib-toml.json >/dev/null || {
    echo "std.toml performance: parent registry drift" >&2
    exit 1
}

for marker in \
    'STD-TOML-PERF-001' \
    'logical allocations' \
    'modeled logical memory' \
    'tail latency' \
    'not a hosted VM' \
    'not-claimed' \
    'stdlib-toml-performance.json'; do
    grep -Fq "$marker" docs/contracts/stdlib-toml-performance.md || {
        echo "std.toml performance: document misses $marker" >&2
        exit 1
    }
done
grep -Fq 'stdlib-toml-performance.json' TONDO_STANDARD_LIBRARY_SPEC.md
grep -Fq 'stdlib-toml-performance.md' docs/contracts/stdlib-toml.md
grep -Fq 'STD-TOML-PERF-001' TONDO_IMPLEMENTATION_TRACKER.md

echo "std.toml performance contract: OK (13 kernel workloads; native and hosted claims excluded)"
