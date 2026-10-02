#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/stdlib-cbor-conformance-check.sh
python3 -B scripts/stdlib_cbor_conformance_test.py
cargo test -q -p tondo-reliability --example cbor_conformance_vm --locked
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-cbor-conformance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
expect_failure() {
    local name="$1"
    shift
    if "$@" >"$tmp/$name.log" 2>&1; then
        echo "std.cbor conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}
for status in adapter-ready verified-hosted-vm-adapter-and-native-stdlib-process; do
    jq --arg status "$status" '.status = $status' testing/stdlib-cbor-conformance.json >"$tmp/conformance.json"
    jq --arg status "$status" '
      .conformance.status = $status
      | if $status == "adapter-ready" then
        .promotion.next_blocks = ["STD-CBOR-CONF-001"]
        | .implementation.required_follow_ups = ["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
      else
        .promotion.next_blocks = ["STD-CBOR-DOC-001"]
        | .implementation.required_follow_ups = ["STD-CBOR-DOC-001"]
      end
    ' testing/stdlib-cbor.json >"$tmp/parent.json"
    for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check stdlib-cbor-conformance-check; do
        env TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh" >/dev/null
    done
    for mutation in \
      'missing-link|del(.conformance.contract)' \
      'wrong-next|.promotion.next_blocks=[]' \
      'wrong-followups|.implementation.required_follow_ups=[]' \
      'public-api|.conformance.public_api="verified"' \
      'native-aot|.conformance.native_aot="verified"'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" >"$tmp/invalid.json"
        for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check stdlib-cbor-conformance-check; do
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_CBOR_CONTRACT="$tmp/invalid.json" TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh"
        done
    done
done
jq '.status="adapter-ready"' testing/stdlib-cbor-conformance.json >"$tmp/conformance.json"
expect_failure mismatched-status env TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT="$tmp/conformance.json" scripts/stdlib-cbor-conformance-check.sh
for override in RUSTC RUSTC_WRAPPER CARGO_BUILD_TARGET CARGO_PROFILE_DEV_OPT_LEVEL; do
    expect_failure "$override" env TONDO_STDLIB_CBOR_CONF_ALLOW_DIRTY=1 "$override=unrecorded" scripts/stdlib-cbor-conformance.sh
    grep -Fq 'unsupported build override' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
echo "std.cbor conformance tests: OK (exact observations, provenance, lifecycle and both owner transitions)"
