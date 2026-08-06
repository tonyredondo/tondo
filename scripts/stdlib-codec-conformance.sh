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

revision="$(git rev-parse HEAD)"
jq -n \
    --arg revision "$revision" \
    '{format:"tondo-stdlib-codec-conformance/1",revision:$revision,oracle:"portable-kernel",bridge:"hosted-vm",modules:["json","messagepack","protobuf"],cases:["valid-round-trip","canonical-order","truncated-input","duplicate-or-invalid-wire","unknown-field-preservation"],status:"passed"}' \
    > "$evidence_dir/stdlib-codec-conformance.json"

echo "stdlib codec conformance: OK"
