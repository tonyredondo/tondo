#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
mkdir -p "$evidence_dir"

run_case() {
    local name="$1"
    shift
    echo "::group::$name"
    "$@"
    echo "::endgroup::"
}

run_case json-kernel cargo test -p tondo-stdlib --lib json::tests --locked
run_case messagepack-kernel cargo test -p tondo-stdlib --lib messagepack::tests --locked
run_case protobuf-kernel cargo test -p tondo-stdlib --lib protobuf::tests --locked
run_case protobuf-owner cargo test -p tondo-stdlib --lib protobuf::protobuf_api::tests --locked
run_case hosted-bridge cargo test -p tondo-compiler --lib hosted_codecs_validate_and_canonicalize_without_partial_success --locked
run_case external-interop cargo test -p tondo-stdlib --test codec_conformance --locked --no-fail-fast

contract="testing/stdlib-codec-conformance.json"
if [[ ! -f "$contract" ]]; then
    echo "missing codec conformance contract: $contract" >&2
    exit 1
fi
if ! tail -c 1 "$contract" | cmp -s <(printf '\n'); then
    echo "codec conformance contract must end with one LF" >&2
    exit 1
fi
if grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null; then
    echo "codec conformance contract contains CR or trailing whitespace" >&2
    exit 1
fi
jq -e '
    .format == "tondo-stdlib-codec-conformance/1"
    and .edition == "0.1"
    and .owner == "std.serialization"
    and .status == "closed"
    and .test.source == "crates/tondo-stdlib/tests/codec_conformance.rs"
    and .test.directions == ["external-to-tondo", "tondo-to-external"]
    and .test.fragmentation == "one-byte-chunks"
    and .test.malformed_inputs == true
    and .test.truncation == true
    and .test.finite_limits == true
    and .test.unknown_preservation == true
    and ([.implementations.json[].id] | unique) == ["serde_json"]
    and ([.implementations.messagepack[].id] | unique) == ["rmpv"]
    and ([.implementations.protobuf[].id] | unique) == ["prost"]
    and all(.implementations[][]; (.id | length) > 0 and (.version | length) > 0 and (.role | length) > 0 and (.source | startswith("https://crates.io/crates/")))
    and ([.cases[].id] | length) == 6
    and ([.cases[].id] | unique | length) == 6
    and all(.cases[]; (.format | IN("json", "messagepack", "protobuf")) and (.observables | length) > 0)
' "$contract" >/dev/null

revision="$(git rev-parse HEAD)"
jq -n \
    --arg revision "$revision" \
    '{format:"tondo-stdlib-codec-conformance/1",revision:$revision,oracle:"independent-external-implementations",implementations:{json:"serde_json@1.0.151",messagepack:"rmpv@1.3.1",protobuf:"prost@0.14.4"},modules:["json","messagepack","protobuf"],cases:["bidirectional-wire", "one-byte-fragments", "malformed-and-truncated", "finite-limits", "unknown-preservation"],test_source:"crates/tondo-stdlib/tests/codec_conformance.rs",status:"passed"}' \
    > "$evidence_dir/stdlib-codec-conformance.json"

echo "stdlib codec conformance: OK"
