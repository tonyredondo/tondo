#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence="target/reliability/evidence"
logs="$evidence/logs"
mkdir -p "$logs"

sanitize() {
    sed "s#${root}/#./#g"
}

run_step() {
    local name="$1"
    shift
    echo "::group::$name"
    "$@" 2>&1 | sanitize | tee "$logs/$name.log"
    echo "::endgroup::"
}

jq -n \
    --arg format "tondo-ci-evidence/1" \
    --arg target "${TONDO_TEST_TARGET:-host-native}" \
    --arg rustc "$(rustc --version)" \
    --arg cargo "$(cargo --version)" \
    --arg seed "${TONDO_TEST_SEED:-deterministic}" \
    '{format:$format,target:$target,rustc:$rustc,cargo:$cargo,seed:$seed}' \
    > "$evidence/metadata.json"

draft_manifest_hash="$(sha256sum conformance/draft/manifest.json | cut -d ' ' -f 1)"
cp conformance/draft/manifest.json \
    "$evidence/draft-manifest-$draft_manifest_hash.json"

run_step fmt cargo fmt --all -- --check
run_step check cargo check --workspace --all-targets --locked
run_step clippy cargo clippy --workspace --all-targets --locked -- -D warnings
run_step test cargo test --workspace --all-targets --locked
run_step rustdoc env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps --locked
run_step conformance-build \
    cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
run_step reliability \
    cargo run -p tondo-reliability --locked -- check --root .
run_step ratchet \
    cargo run -p tondo-reliability --locked -- ratchet check --root .
run_step draft-lineage-validate \
    cargo run -p tondo-conformance --locked -- validate \
    --root . \
    --manifest conformance/draft/manifest.json \
    --lineage draft
run_step conformance-run \
    cargo run -p tondo-conformance --locked -- run \
    --root . \
    --manifest conformance/draft/manifest.json \
    --lineage draft \
    --adapter target/debug/tondo-reference-adapter \
    --output "$evidence/conformance-result.json"
run_step conformance-compare \
    cmp "$evidence/conformance-result.json" \
    conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json
