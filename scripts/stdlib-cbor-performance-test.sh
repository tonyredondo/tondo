#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-cbor-performance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.cbor performance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

scripts/stdlib-cbor-performance-check.sh
python3 -B scripts/stdlib_cbor_performance_test.py
cargo test -q -p tondo-stdlib --test cbor_performance --locked

# Check both legitimate progression states before metadata-only closure. This
# also exercises the parent/implementation consumers of the shared jq rule.
for status in measurement-ready verified-stdlib-kernel-baseline; do
    jq --arg status "$status" '.status = $status' testing/stdlib-cbor-performance.json > "$tmp/performance.json"
    jq --arg status "$status" '
      .performance.status = $status
      | del(.conformance)
      | if $status == "verified-stdlib-kernel-baseline" then
          .promotion.next_blocks = ["STD-CBOR-CONF-001"]
          | .implementation.required_follow_ups = ["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
        else
          .promotion.next_blocks = ["STD-CBOR-PERF-001"]
          | .implementation.required_follow_ups = ["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
        end
    ' testing/stdlib-cbor.json > "$tmp/parent.json"
    env TONDO_STDLIB_CBOR_PERF_CONTRACT="$tmp/performance.json" TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" \
        scripts/stdlib-cbor-performance-check.sh >/dev/null
    env TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" scripts/stdlib-cbor-check.sh >/dev/null
    env TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" scripts/stdlib-cbor-implementation-check.sh >/dev/null
    for mutation in \
        'stale-next|.promotion.next_blocks = ["STD-CBOR-DOC-001"]' \
        'missing-followup|.implementation.required_follow_ups = []' \
        'missing-test-link|del(.testing_contract)' \
        'missing-perf-link|del(.performance.contract)' \
        'mixed-child-state|.performance.status = "open"' \
        'public-claim|.implementation.public_api_promoted = true' \
        'native-claim|.performance.native_aot = "verified"'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid-parent.json"
        for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check; do
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_CBOR_PERF_CONTRACT="$tmp/performance.json" \
                TONDO_STDLIB_CBOR_CONTRACT="$tmp/invalid-parent.json" "scripts/$checker.sh"
        done
    done
done
jq '.status = "measurement-ready"' testing/stdlib-cbor-performance.json > "$tmp/performance.json"
expect_failure mixed-parent-status env TONDO_STDLIB_CBOR_PERF_CONTRACT="$tmp/performance.json" \
    TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" scripts/stdlib-cbor-performance-check.sh
for override in RUSTC_WRAPPER CARGO_PROFILE_TEST_OPT_LEVEL CARGO_BUILD_TARGET \
    CARGO_BUILD_RUSTFLAGS CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS; do
    expect_failure "$override" env TONDO_STDLIB_CBOR_PERF_ALLOW_DIRTY=1 "$override=unrecorded" \
        scripts/stdlib-cbor-performance.sh
    grep -Fq 'unsupported' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
echo "std.cbor performance tests: OK (retained reports, provenance, resource boundaries, lifecycle and both owner transitions)"
