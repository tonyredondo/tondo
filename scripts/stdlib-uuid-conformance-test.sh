#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
scripts/stdlib-uuid-conformance-check.sh
python3 -B scripts/stdlib_uuid_conformance_test.py
cargo test -q -p tondo-reliability --example uuid_conformance_vm --locked
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-uuid-conformance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
negative_count=0
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp/$name.log" 2>&1; then
        echo "std.uuid conformance: $name unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count+1))
}
for status in adapter-ready verified-public-hosted-vm-and-native-kernel-process; do
    jq --arg status "$status" '.status=$status' testing/stdlib-uuid-conformance.json > "$tmp/conformance.json"
    jq --arg status "$status" '
      .conformance.status=$status
      | .conformance.quality_gate=(if $status == "adapter-ready" then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
      | .promotion.next_blocks=(if $status == "adapter-ready" then ["STD-UUID-CONF-001"] else ["STD-UUID-DOC-001"] end)
      | .implementation.required_follow_ups=(if $status == "adapter-ready" then ["STD-UUID-CONF-001", "STD-UUID-DOC-001"] else ["STD-UUID-DOC-001"] end)
    ' testing/stdlib-uuid.json > "$tmp/parent.json"
    for checker in stdlib-uuid-check stdlib-uuid-implementation-check stdlib-uuid-host-check \
        stdlib-uuid-test-check stdlib-uuid-performance-check stdlib-uuid-conformance-check; do
        env TONDO_STDLIB_UUID_CONTRACT="$tmp/parent.json" TONDO_STDLIB_UUID_CONFORMANCE_CONTRACT="$tmp/conformance.json" \
            "scripts/$checker.sh" >/dev/null
        for mutation in \
            'missing-link|del(.conformance.register)' \
            'premature-next|.promotion.next_blocks=[]' \
            'lost-followups|.implementation.required_follow_ups=[]' \
            'mixed-state|.conformance.status="published"' \
            'mixed-quality|.conformance.quality_gate="passed"' \
            'private-callback|.conformance.public_api="test-only-host-callable"' \
            'missing-case|.conformance.cases=4' \
            'missing-observation|.conformance.common_observations=76' \
            'missing-static-proof|.conformance.vm_capability_checks=9' \
            'native-ABI|.conformance.native_abi="verified"' \
            'native-AOT|.conformance.native_aot="verified"' \
            'wrong-target|.conformance.target="aarch64-unknown-linux-gnu"' \
            'unverified-model|.model.status="ready"' \
            'unverified-performance|.measurement.status="measurement-ready"'; do
            name="${mutation%%|*}"
            jq "${mutation#*|}" "$tmp/parent.json" > "$tmp/invalid-parent.json"
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_UUID_CONTRACT="$tmp/invalid-parent.json" \
                TONDO_STDLIB_UUID_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh"
        done
    done
done
jq '.status="adapter-ready"' testing/stdlib-uuid-conformance.json > "$tmp/conformance.json"
expect_failure mixed-child-status env TONDO_STDLIB_UUID_CONTRACT="$tmp/parent.json" \
    TONDO_STDLIB_UUID_CONFORMANCE_CONTRACT="$tmp/conformance.json" scripts/stdlib-uuid-conformance-check.sh
for override in RUSTC_WRAPPER CARGO_BUILD_TARGET CARGO_PROFILE_DEV_OPT_LEVEL \
    CARGO_PROFILE_TEST_OPT_LEVEL CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER; do
    expect_failure "$override" env TONDO_STDLIB_UUID_CONF_ALLOW_DIRTY=1 "$override=unrecorded" \
        scripts/stdlib-uuid-conformance.sh
    grep -Fq 'unsupported build override' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
echo "std.uuid conformance tests: OK ($negative_count refusals; exact outputs, lifecycle, provenance and both owner states)"
