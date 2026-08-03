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
run_step stdlib-performance-contract \
    scripts/stdlib-performance-check.sh
run_step stdlib-json-contract \
    scripts/stdlib-json-check.sh
run_step stdlib-messagepack-contract \
    scripts/stdlib-messagepack-check.sh
run_step stdlib-protobuf-contract \
    scripts/stdlib-protobuf-check.sh
run_step stdlib-testing-contract \
    scripts/stdlib-testing-check.sh
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

candidate_directory="$(mktemp -d "$evidence/candidate.XXXXXX")"
rmdir "$candidate_directory"
run_step conformance-seal \
    cargo run -p tondo-conformance --locked -- seal \
    --root . \
    --manifest conformance/draft/manifest.json \
    --lineage draft \
    --result "$evidence/conformance-result.json" \
    --output "$candidate_directory"
run_step conformance-candidate-compare \
    cmp "$candidate_directory/manifest.json" conformance/candidate/manifest.json
run_step conformance-candidate-verify \
    cargo run -p tondo-conformance --locked -- verify-candidate \
    --root . \
    --candidate conformance/candidate
