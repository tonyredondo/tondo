#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-regex-doc-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
expect_failure() {
    local name="$1"
    shift
    if "$@" >"$tmp/$name.log" 2>&1; then
        echo "std.regex documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

scripts/stdlib-regex-doc-check.sh
for section in 'Patterns and reuse' 'Unicode and options' 'Captures and UTF-8 spans' \
    'Lazy iteration and ownership' 'Replacement and errors' 'Limits and costs' \
    'Executable kernel example' 'Promotion boundary'; do
    sed "/^### $section$/d" docs/contracts/stdlib-regex.md >"$tmp/missing-section.md"
    expect_failure missing-section env TONDO_STDLIB_REGEX_DOCUMENT="$tmp/missing-section.md" \
        scripts/stdlib-regex-doc-check.sh
done
for marker in 'does not return the earlier' 'system OOM' 'not claimed globally linear'; do
    grep -Fv "$marker" docs/contracts/stdlib-regex.md >"$tmp/missing-cost.md"
    expect_failure missing-cost env TONDO_STDLIB_REGEX_DOCUMENT="$tmp/missing-cost.md" \
        scripts/stdlib-regex-doc-check.sh
done
for status in usage-ready verified-rust-kernel-usage; do
    jq --arg status "$status" '
      .documentation.status = $status
      | if $status == "usage-ready" then
        .implementation.required_follow_ups = ["STD-REGEX-DOC-001"]
        | .promotion.next_blocks = ["STD-REGEX-DOC-001"]
      else
        .implementation.required_follow_ups = []
        | .promotion.next_blocks = ["STD-UUID-IMPL-001"]
      end
    ' testing/stdlib-regex.json >"$tmp/parent.json"
    for checker in stdlib-regex-check stdlib-regex-implementation-check \
        stdlib-regex-performance-check stdlib-regex-conformance-check stdlib-regex-doc-check; do
        env TONDO_STDLIB_REGEX_CONTRACT="$tmp/parent.json" "scripts/$checker.sh" >/dev/null
    done
    for mutation in \
        'status|.documentation.status="pending"' \
        'example-path|.documentation.example="other.rs"' \
        'missing-example|del(.documentation.example)' \
        'command|.documentation.command="true"' \
        'stdout|.documentation.expected_stdout="wrong"' \
        'examples|.documentation.examples=.documentation.examples[0:5]' \
        'sections|.documentation.sections=.documentation.sections[0:7]' \
        'public-api|.documentation.public_tondo_api="verified"' \
        'production-vm|.documentation.production_vm_registration="verified"' \
        'native-abi|.documentation.native_abi="verified"' \
        'native-aot|.documentation.native_aot="verified"' \
        'production-host|.implementation.host="verified"' \
        'native-lowering|.implementation.native_aot_lowering="verified"' \
        'wrong-next|.promotion.next_blocks=["STD-REGEX-CONF-001"]' \
        'wrong-followups|.implementation.required_follow_ups=["STD-REGEX-IMPL-001"]'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" >"$tmp/invalid.json"
        for checker in stdlib-regex-check stdlib-regex-implementation-check \
            stdlib-regex-performance-check stdlib-regex-conformance-check stdlib-regex-doc-check; do
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_REGEX_CONTRACT="$tmp/invalid.json" "scripts/$checker.sh"
        done
    done
done
for prerequisite in absent adapter-ready; do
    jq --arg prerequisite "$prerequisite" '
      (if $prerequisite == "absent" then del(.conformance)
        else .conformance.status="adapter-ready" end)
      | .implementation.required_follow_ups=["STD-REGEX-CONF-001","STD-REGEX-DOC-001"]
      | .promotion.next_blocks=["STD-REGEX-CONF-001"]
    ' testing/stdlib-regex.json >"$tmp/early.json"
    jq '.status="adapter-ready"' testing/stdlib-regex-conformance.json >"$tmp/conformance.json"
    for checker in stdlib-regex-check stdlib-regex-implementation-check \
        stdlib-regex-performance-check stdlib-regex-conformance-check stdlib-regex-doc-check; do
        expect_failure "early-$prerequisite-$checker" env TONDO_STDLIB_REGEX_CONTRACT="$tmp/early.json" \
            TONDO_STDLIB_REGEX_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh"
    done
done
bash -n scripts/stdlib-regex-doc-check.sh scripts/stdlib-regex-doc-test.sh
echo "std.regex documentation tests: OK (eight sections, canonical example, costs, boundaries and both owner transitions)"
