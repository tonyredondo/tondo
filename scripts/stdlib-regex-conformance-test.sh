#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
scripts/stdlib-regex-conformance-check.sh
python3 -B scripts/stdlib_regex_conformance_test.py
cargo test -q -p tondo-reliability --example regex_conformance_vm --locked
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-regex-conformance-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
expect_failure() {
    local name="$1"
    shift
    if "$@" >"$tmp/$name.log" 2>&1; then
        echo "std.regex conformance tests: $name unexpectedly passed" >&2
        exit 1
    fi
}
for status in adapter-ready verified-hosted-vm-adapter-and-native-stdlib-process; do
    jq --arg status "$status" '.status = $status' testing/stdlib-regex-conformance.json >"$tmp/conformance.json"
    jq --arg status "$status" '
      .conformance.status = $status
      | del(.documentation)
      | if $status == "adapter-ready" then
        .promotion.next_blocks = ["STD-REGEX-CONF-001"]
        | .implementation.required_follow_ups = ["STD-REGEX-CONF-001","STD-REGEX-DOC-001"]
      else
        .promotion.next_blocks = ["STD-REGEX-DOC-001"]
        | .implementation.required_follow_ups = ["STD-REGEX-DOC-001"]
      end
    ' testing/stdlib-regex.json >"$tmp/parent.json"
    for checker in stdlib-regex-check stdlib-regex-implementation-check stdlib-regex-performance-check stdlib-regex-conformance-check; do
        env TONDO_STDLIB_REGEX_CONTRACT="$tmp/parent.json" TONDO_STDLIB_REGEX_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh" >/dev/null
    done
    for mutation in \
      'missing-link|del(.conformance.contract)' \
      'wrong-next|.promotion.next_blocks=[]' \
      'wrong-followups|.implementation.required_follow_ups=[]' \
      'public-api|.conformance.public_api="verified"' \
      'native-aot|.conformance.native_aot="verified"'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" >"$tmp/invalid.json"
        for checker in stdlib-regex-check stdlib-regex-implementation-check stdlib-regex-performance-check stdlib-regex-conformance-check; do
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_REGEX_CONTRACT="$tmp/invalid.json" TONDO_STDLIB_REGEX_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh"
        done
    done
done
jq '.status="adapter-ready"' testing/stdlib-regex-conformance.json >"$tmp/conformance.json"
expect_failure mismatched-status env TONDO_STDLIB_REGEX_CONTRACT="$tmp/parent.json" TONDO_STDLIB_REGEX_CONFORMANCE_CONTRACT="$tmp/conformance.json" scripts/stdlib-regex-conformance-check.sh
for override in RUSTC RUSTC_WRAPPER CARGO_BUILD_TARGET CARGO_PROFILE_DEV_OPT_LEVEL; do
    expect_failure "$override" env TONDO_STDLIB_REGEX_CONF_ALLOW_DIRTY=1 "$override=unrecorded" scripts/stdlib-regex-conformance.sh
    grep -Fq 'unsupported build override' "$tmp/$override.log" || { cat "$tmp/$override.log" >&2; exit 1; }
done
echo "std.regex conformance tests: OK (exact observations, provenance, lifecycle and both owner transitions)"
