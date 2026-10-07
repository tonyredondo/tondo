#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-net-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
reject() {
    if "$@" > "$tmp/refused.log" 2>&1; then
        echo 'std.net performance: invalid input accepted' >&2; exit 1
    fi
}
bash scripts/stdlib-net-performance-check.sh
python3 -B scripts/stdlib_net_performance_test.py
cargo test -p tondo-compiler --lib --locked process_host::net::performance
for status in measurement-ready verified-hosted-scalar-baseline; do
    jq --arg status "$status" '.status=$status' testing/stdlib-net-performance.json > "$tmp/child.json"
    jq --arg status "$status" '
      .measurement.status=$status
      | .measurement.quality_gate=(if $status=="measurement-ready"
          then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status=="measurement-ready"
          then ["STD-NET-PERF-001"] else ["STD-NET-CONF-001"] end)
    ' testing/stdlib-net.json > "$tmp/parent.json"
    env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" TONDO_STDLIB_NET_PERF_CONTRACT="$tmp/child.json" \
        bash scripts/stdlib-net-performance-check.sh
    for mutation in '.measurement.samples_per_workload=9' '.measurement.native_aot="verified"' \
        '.measurement.hosted_vm_timing="verified"' '.model.status="ready"' \
        '.measurement.quality_gate="passed"' '.promotion.next_blocks=[]'; do
        jq "$mutation" "$tmp/parent.json" > "$tmp/invalid-parent.json"
        reject env TONDO_STDLIB_NET_CONTRACT="$tmp/invalid-parent.json" \
            TONDO_STDLIB_NET_PERF_CONTRACT="$tmp/child.json" bash scripts/stdlib-net-performance-check.sh
    done
done
for mutation in '.probe.sha256="stale"' '.strategy.native_aot="verified"' '.protocol.outliers="delete"' \
    '.protocol.independent_processes=1' '.workloads|=.[0:20]' '.resource_model.logical_memory="RSS"' \
    '.target="aarch64-unknown-linux-gnu"' '.identity_fields+=["pid"]' '.workloads[0].payload_bytes=1'; do
    jq "$mutation" testing/stdlib-net-performance.json > "$tmp/invalid-child.json"
    reject env TONDO_STDLIB_NET_PERF_CONTRACT="$tmp/invalid-child.json" bash scripts/stdlib-net-performance-check.sh
done
for override in RUSTC_WRAPPER CARGO_PROFILE_TEST_OPT_LEVEL CARGO_BUILD_TARGET \
    CARGO_BUILD_RUSTFLAGS CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS; do
    reject env TONDO_STDLIB_NET_PERF_ALLOW_DIRTY=1 "$override=unrecorded" bash scripts/stdlib-net-performance.sh
    grep -Fq 'unsupported' "$tmp/refused.log"
done
reject env TONDO_NET_PERF_RUN=1 TONDO_NET_PERF_PROCESS=4 \
    cargo test -q -p tondo-compiler --lib --locked \
    process_host::net::performance::network_performance_probe -- --exact
echo 'std.net performance tests: OK (retained reports, real fixtures, phase transitions and provenance refusals)'
