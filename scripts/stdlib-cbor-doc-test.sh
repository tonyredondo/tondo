#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp="$(mktemp -d "${TMPDIR:-/tmp}/tondo-cbor-doc-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
expect_failure() {
    local name="$1"
    shift
    if "$@" >"$tmp/$name.log" 2>&1; then
        echo "std.cbor documentation tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

scripts/stdlib-cbor-doc-check.sh
for section in 'Values and byte preservation' 'Tags and deterministic encoding' \
    'Policies and limits' 'Errors, ownership and terminal streams' \
    'Costs and performance' 'Executable kernel example' 'Promotion boundary'; do
    sed "/^### $section$/d" docs/contracts/stdlib-cbor.md >"$tmp/missing-section.md"
    expect_failure missing-section env TONDO_STDLIB_CBOR_DOCUMENT="$tmp/missing-section.md" \
        scripts/stdlib-cbor-doc-check.sh
done
for marker in 'External I/O is not transactional' 'including after terminal `finish()`'; do
    grep -Fv "$marker" docs/contracts/stdlib-cbor.md >"$tmp/missing-cost.md"
    expect_failure missing-cost env TONDO_STDLIB_CBOR_DOCUMENT="$tmp/missing-cost.md" \
        scripts/stdlib-cbor-doc-check.sh
done
for status in usage-ready verified-rust-kernel-usage; do
    jq --arg status "$status" '
      .documentation.status = $status
      | if $status == "usage-ready" then
        .implementation.required_follow_ups = ["STD-CBOR-DOC-001"]
        | .promotion.next_blocks = ["STD-CBOR-DOC-001"]
      else
        .implementation.required_follow_ups = []
        | .promotion.next_blocks = ["STD-REGEX-IMPL-001"]
      end
    ' testing/stdlib-cbor.json >"$tmp/parent.json"
    for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check \
        stdlib-cbor-conformance-check stdlib-cbor-doc-check; do
        env TONDO_STDLIB_CBOR_CONTRACT="$tmp/parent.json" "scripts/$checker.sh" >/dev/null
    done
    for mutation in \
        'status|.documentation.status="pending"' \
        'example-path|.documentation.example="other.rs"' \
        'missing-example|del(.documentation.example)' \
        'command|.documentation.command="true"' \
        'stdout|.documentation.expected_stdout="wrong"' \
        'examples|.documentation.examples=.documentation.examples[0:5]' \
        'sections|.documentation.sections=.documentation.sections[0:6]' \
        'public-api|.documentation.public_tondo_api="verified"' \
        'native-aot|.documentation.native_aot="verified"' \
        'production-host|.implementation.host="verified"' \
        'native-lowering|.implementation.native_aot_lowering="verified"' \
        'wrong-next|.promotion.next_blocks=["STD-CBOR-CONF-001"]' \
        'wrong-followups|.implementation.required_follow_ups=["STD-CBOR-IMPL-001"]'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp/parent.json" >"$tmp/invalid.json"
        for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check \
            stdlib-cbor-conformance-check stdlib-cbor-doc-check; do
            expect_failure "$status-$name-$checker" env TONDO_STDLIB_CBOR_CONTRACT="$tmp/invalid.json" "scripts/$checker.sh"
        done
    done
done
# A documentation record cannot skip an unfinished prerequisite.
jq '.conformance.status="adapter-ready" | .implementation.required_follow_ups=["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
    | .promotion.next_blocks=["STD-CBOR-CONF-001"]' testing/stdlib-cbor.json >"$tmp/early.json"
jq '.status="adapter-ready"' testing/stdlib-cbor-conformance.json >"$tmp/conformance.json"
for checker in stdlib-cbor-check stdlib-cbor-implementation-check stdlib-cbor-performance-check \
    stdlib-cbor-conformance-check stdlib-cbor-doc-check; do
    expect_failure "early-$checker" env TONDO_STDLIB_CBOR_CONTRACT="$tmp/early.json" \
        TONDO_STDLIB_CBOR_CONFORMANCE_CONTRACT="$tmp/conformance.json" "scripts/$checker.sh"
done
bash -n scripts/stdlib-cbor-doc-check.sh scripts/stdlib-cbor-doc-test.sh
echo "std.cbor documentation tests: OK (seven sections, canonical source, boundaries and both owner transitions)"
