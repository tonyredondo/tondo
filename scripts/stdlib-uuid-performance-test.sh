#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-uuid-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
negative_count=0
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.uuid performance tests: $name unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count + 1))
}
scripts/stdlib-uuid-performance-check.sh
python3 -B scripts/stdlib_uuid_performance_test.py
cargo test -q -p tondo-compiler --lib --locked process_host::uuid::performance
for status in measurement-ready verified-hosted-scalar-baseline; do
    jq --arg status "$status" '.status=$status' testing/stdlib-uuid-performance.json > "$tmp/performance.json"
    jq --arg status "$status" '
      del(.conformance, .documentation)
      | .measurement.status=$status
      | .measurement.quality_gate=(if $status == "measurement-ready" then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status == "measurement-ready" then ["STD-UUID-PERF-001"] else ["STD-UUID-CONF-001"] end)
      | .implementation.required_follow_ups=(if $status == "measurement-ready" then ["STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"] else ["STD-UUID-CONF-001", "STD-UUID-DOC-001"] end)
    ' testing/stdlib-uuid.json > "$tmp/parent.json"
    for checker in stdlib-uuid-check stdlib-uuid-implementation-check stdlib-uuid-host-check stdlib-uuid-test-check stdlib-uuid-performance-check; do
        env TONDO_STDLIB_UUID_PERF_CONTRACT="$tmp/performance.json" TONDO_STDLIB_UUID_CONTRACT="$tmp/parent.json" \
            "scripts/$checker.sh" >/dev/null
        for mutation in \
            'stale-next|.promotion.next_blocks=["STD-UUID-DOC-001"]' \
            'lost-followup|.implementation.required_follow_ups=[]' \
            'missing-link|del(.measurement.register)' \
            'mixed-child-state|.measurement.status="published"' \
            'mixed-quality|.measurement.quality_gate="passed"' \
            'wrong-target|.measurement.target="aarch64-unknown-linux-gnu"' \
            'lost-processes|.measurement.samples_per_workload=9' \
            'ambient-route|.measurement.pid=12345' \
            'VM-timing-claim|.measurement.hosted_vm_timing="verified"' \
            'native-claim|.measurement.native_aot="verified"' \
            'unverified-model|.model.status="ready"'; do
            name="${mutation%%|*}"
            jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid-parent.json"
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_UUID_PERF_CONTRACT="$tmp/performance.json" \
                TONDO_STDLIB_UUID_CONTRACT="$tmp/invalid-parent.json" "scripts/$checker.sh"
        done
    done
done
jq '.status="measurement-ready"' testing/stdlib-uuid-performance.json > "$tmp/performance.json"
expect_failure mixed-parent-status env TONDO_STDLIB_UUID_PERF_CONTRACT="$tmp/performance.json" \
    TONDO_STDLIB_UUID_CONTRACT="$tmp/parent.json" scripts/stdlib-uuid-performance-check.sh
for override in RUSTC_WRAPPER CARGO_PROFILE_TEST_OPT_LEVEL CARGO_BUILD_TARGET \
    CARGO_BUILD_RUSTFLAGS CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS; do
    expect_failure "$override" env TONDO_STDLIB_UUID_PERF_ALLOW_DIRTY=1 "$override=unrecorded" \
        scripts/stdlib-uuid-performance.sh
    grep -Fq 'unsupported' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
expect_failure invalid-process env TONDO_UUID_PERF_RUN=1 TONDO_UUID_PERF_PROCESS=4 \
    cargo test -q -p tondo-compiler --lib --locked process_host::uuid::performance::uuid_performance_probe -- --exact
echo "std.uuid performance tests: OK ($negative_count refusals; reports, exact oracles, lifecycle, provenance, transitions)"
