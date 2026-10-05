#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-regex-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.regex performance tests: $name unexpectedly passed" >&2; exit 1
    fi
}

scripts/stdlib-regex-performance-check.sh
python3 -B scripts/stdlib_regex_performance_test.py
cargo test -q -p tondo-stdlib --lib regex::performance --locked
for status in measurement-ready verified-stdlib-kernel-baseline; do
    jq --arg status "$status" '.status = $status' testing/stdlib-regex-performance.json > "$tmp/performance.json"
    jq --arg status "$status" '
      .performance.status = $status
      | del(.conformance, .documentation)
      | if $status == "verified-stdlib-kernel-baseline" then
          .promotion.next_blocks = ["STD-REGEX-CONF-001"]
          | .implementation.required_follow_ups = ["STD-REGEX-CONF-001","STD-REGEX-DOC-001"]
        else
          .promotion.next_blocks = ["STD-REGEX-PERF-001"]
          | .implementation.required_follow_ups = ["STD-REGEX-PERF-001","STD-REGEX-CONF-001","STD-REGEX-DOC-001"]
        end
    ' testing/stdlib-regex.json > "$tmp/parent.json"
    for checker in stdlib-regex-check stdlib-regex-implementation-check stdlib-regex-performance-check; do
        env TONDO_STDLIB_REGEX_PERF_CONTRACT="$tmp/performance.json" TONDO_STDLIB_REGEX_CONTRACT="$tmp/parent.json" \
            "scripts/$checker.sh" >/dev/null
        for mutation in \
            'stale-next|.promotion.next_blocks = ["STD-REGEX-DOC-001"]' \
            'missing-followup|.implementation.required_follow_ups = []' \
            'missing-test-link|del(.testing_contract)' \
            'missing-perf-link|del(.performance.contract)' \
            'mixed-child-state|.performance.status = "open"' \
            'wrong-target|.performance.target = "aarch64-unknown-linux-gnu"' \
            'lost-processes|.performance.samples_per_workload = 9' \
            'ambient-route|.performance.pid = 12345' \
            'public-claim|.implementation.public_api_promoted = true' \
            'vm-claim|.performance.hosted_vm = "verified"' \
            'native-claim|.performance.native_aot = "verified"'; do
            name="${mutation%%|*}"
            jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid-parent.json"
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_REGEX_PERF_CONTRACT="$tmp/performance.json" \
                TONDO_STDLIB_REGEX_CONTRACT="$tmp/invalid-parent.json" "scripts/$checker.sh"
        done
    done
done
jq '.status = "measurement-ready"' testing/stdlib-regex-performance.json > "$tmp/performance.json"
expect_failure mixed-parent-status env TONDO_STDLIB_REGEX_PERF_CONTRACT="$tmp/performance.json" \
    TONDO_STDLIB_REGEX_CONTRACT="$tmp/parent.json" scripts/stdlib-regex-performance-check.sh
for override in RUSTC_WRAPPER CARGO_PROFILE_TEST_OPT_LEVEL CARGO_BUILD_TARGET \
    CARGO_BUILD_RUSTFLAGS CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS; do
    expect_failure "$override" env TONDO_STDLIB_REGEX_PERF_ALLOW_DIRTY=1 "$override=unrecorded" \
        scripts/stdlib-regex-performance.sh
    grep -Fq 'unsupported' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
echo "std.regex performance tests: OK (retained reports, exact oracles, lifecycle, provenance and owner transitions)"
