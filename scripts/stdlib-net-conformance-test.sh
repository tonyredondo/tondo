#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
scripts/stdlib-net-conformance-check.sh
python3 -B scripts/stdlib_net_conformance_test.py
cargo test -q -p tondo-reliability --example net_conformance_vm --locked
cargo test -q -p tondo-native-runtime --example net_conformance --locked
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-net-conformance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
negative_count=0
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.net conformance: $name unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count+1))
}
for status in adapter-ready verified-public-hosted-vm-and-native-kernel-process; do
    jq --arg status "$status" '.status=$status' testing/stdlib-net-conformance.json > "$tmp/conformance.json"
    jq --arg status "$status" '
      .conformance.status=$status
      | .conformance.quality_gate=(if $status=="adapter-ready"
        then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status=="adapter-ready"
        then ["STD-NET-CONF-001"] else ["STD-NET-DOC-001"] end)
    ' testing/stdlib-net.json > "$tmp/parent.json"
    for checker in stdlib-net-check stdlib-net-implementation-check stdlib-net-host-check \
        stdlib-net-test-check stdlib-net-performance-check stdlib-net-conformance-check; do
        env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" \
            TONDO_STDLIB_NET_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh" >/dev/null
    done
    for mutation in \
        'missing-link|del(.conformance.register)' \
        'mixed-status|.conformance.status="published"' \
        'wrong-quality|.conformance.quality_gate="passed"' \
        'wrong-next|.promotion.next_blocks=[]' \
        'private-callback|.conformance.public_api="test-only-host-callable"' \
        'lost-observations|.conformance.common_observations=40' \
        'lost-TLS|.conformance.vm_only_observations=13' \
        'lost-static|.conformance.vm_capability_checks=3' \
        'native-ABI|.conformance.native_abi="verified"' \
        'native-AOT|.conformance.native_aot="verified"' \
        'wrong-target|.conformance.target="aarch64-unknown-linux-gnu"' \
        'unverified-model|.model.status="ready"' \
        'unverified-performance|.measurement.status="measurement-ready"'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid-parent.json"
        expect_failure "$status-$name" env TONDO_STDLIB_NET_CONTRACT="$tmp/invalid-parent.json" \
            TONDO_STDLIB_NET_CONFORMANCE_CONTRACT="$tmp/conformance.json" scripts/stdlib-net-conformance-check.sh
    done
done
jq '.status="adapter-ready"' testing/stdlib-net-conformance.json > "$tmp/conformance.json"
expect_failure mixed-child env TONDO_STDLIB_NET_CONTRACT="$tmp/parent.json" \
    TONDO_STDLIB_NET_CONFORMANCE_CONTRACT="$tmp/conformance.json" scripts/stdlib-net-conformance-check.sh
for override in RUSTC_WRAPPER CARGO_BUILD_TARGET CARGO_PROFILE_DEV_OPT_LEVEL \
    CARGO_PROFILE_TEST_OPT_LEVEL CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER; do
    expect_failure "$override" env TONDO_STDLIB_NET_CONF_ALLOW_DIRTY=1 "$override=unrecorded" \
        scripts/stdlib-net-conformance.sh
    grep -Fq 'unsupported build override' "$tmp/$override.log"
done
expect_failure missing-scope env -u TONDO_TEST_PROCESS_CGROUP TONDO_STDLIB_NET_CONF_ALLOW_DIRTY=1 \
    scripts/stdlib-net-conformance.sh
grep -Fq 'explicit delegated process scope required' "$tmp/missing-scope.log"
expect_failure mismatched-scope env TONDO_TEST_PROCESS_CGROUP=/unmatched-scope \
    TONDO_STDLIB_NET_CONF_ALLOW_DIRTY=1 scripts/stdlib-net-conformance.sh
grep -Fq 'declared process scope differs from actual delegated scope' "$tmp/mismatched-scope.log"
echo "std.net conformance tests: OK ($negative_count refusals; exact reports, lifecycle and provenance)"
