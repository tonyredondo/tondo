#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo_target_dir="${CARGO_TARGET_DIR:-target}"

evidence="$cargo_target_dir/reliability/evidence"
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
run_step stdlib-meta-contract \
    scripts/stdlib-meta-check.sh
run_step stdlib-reflect-contract \
    scripts/stdlib-reflect-check.sh
run_step stdlib-bytes-contract \
    scripts/stdlib-bytes-check.sh
run_step stdlib-time-contract \
    scripts/stdlib-time-check.sh
run_step stdlib-env-contract \
    scripts/stdlib-env-check.sh
run_step stdlib-owner-evidence \
    scripts/stdlib-owner-evidence-check.sh
run_step stdlib-meta-tests \
    scripts/stdlib-meta-test.sh
run_step stdlib-reflect-tests \
    scripts/stdlib-reflect-test.sh
run_step stdlib-bytes-tests \
    scripts/stdlib-bytes-test.sh
run_step stdlib-time-tests \
    scripts/stdlib-time-test.sh
run_step stdlib-env-tests \
    scripts/stdlib-env-test.sh
run_step stdlib-core-contract \
    scripts/stdlib-core-check.sh
run_step stdlib-core-tests \
    scripts/stdlib-core-test.sh
run_step stdlib-text-tests \
    scripts/stdlib-text-test.sh
run_step stdlib-collections-tests \
    scripts/stdlib-collections-test.sh
run_step stdlib-iter-tests \
    scripts/stdlib-iter-test.sh
run_step stdlib-math-tests \
    scripts/stdlib-math-test.sh
run_step stdlib-format-tests \
    scripts/stdlib-format-test.sh
run_step stdlib-io-tests \
    scripts/stdlib-io-test.sh
run_step stdlib-path-tests \
    scripts/stdlib-path-test.sh
run_step stdlib-console-tests \
    scripts/stdlib-console-test.sh
run_step stdlib-fs-tests \
    scripts/stdlib-fs-test.sh
run_step stdlib-process-tests \
    scripts/stdlib-process-test.sh
run_step stdlib-serialization-tests \
    scripts/stdlib-serialization-test.sh
run_step stdlib-serialization-contract \
    scripts/stdlib-serialization-check.sh
run_step stdlib-json-tests \
    scripts/stdlib-json-test.sh
run_step stdlib-messagepack-tests \
    scripts/stdlib-messagepack-test.sh
run_step stdlib-protobuf-tests \
    scripts/stdlib-protobuf-test.sh
run_step stdlib-testing-tests \
    scripts/stdlib-testing-test.sh
run_step stdlib-test-coordination \
    scripts/stdlib-test-coordination-check.sh
run_step stdlib-test-coordination-tests \
    scripts/stdlib-test-coordination-test.sh
run_step stdlib-integration-contract \
    scripts/stdlib-spec-check.sh
run_step stdlib-hosted-contract \
    scripts/stdlib-hosted-check.sh
run_step stdlib-implementation-evidence \
    scripts/stdlib-implementation-check.sh
run_step stdlib-public-api-audit \
    scripts/stdlib-public-api-audit.sh --check
run_step stdlib-public-api-audit-tests \
    scripts/stdlib-public-api-audit-test.sh
run_step stdlib-normative-matrix \
    scripts/stdlib-matrix-check.sh
run_step stdlib-normative-matrix-tests \
    scripts/stdlib-matrix-test.sh
run_step stdlib-codec-conformance \
    scripts/stdlib-codec-conformance.sh
run_step stdlib-performance-report \
    scripts/stdlib-performance-report.sh
run_step stdlib-performance-conformance \
    scripts/stdlib-performance-conformance.sh
run_step stdlib-performance-conformance-tests \
    scripts/stdlib-performance-conformance-test.sh
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
    --adapter "$cargo_target_dir/debug/tondo-reference-adapter" \
    --output "$evidence/conformance-result.json"
run_step conformance-compare \
    cmp "$evidence/conformance-result.json" \
    conformance/0.1/results/tondo-reference-draft-tondo-vm-hosted.json

if jq -e '([.requirements[] | select(.status == "draft-pending")] | length) == 0' \
    testing/coverage-matrix.json >/dev/null; then
    proof_directory="$(mktemp -d "$evidence/promotion-proof.XXXXXX")"
    rmdir "$proof_directory"
    run_step conformance-seal-proof \
        cargo run -p tondo-conformance --locked -- seal-proof \
        --root . \
        --manifest conformance/draft/manifest.json \
        --lineage draft \
        --result "$evidence/conformance-result.json" \
        --output "$proof_directory"
    draft_revision="$(jq -r '.revision' conformance/draft/manifest.json)"
    checked_in_proof="conformance/proofs/revision-$draft_revision"
    run_step conformance-proof-compare \
        cmp "$proof_directory/manifest.json" "$checked_in_proof/manifest.json"
    run_step conformance-proof-verify \
        cargo run -p tondo-conformance --locked -- verify-proof \
        --root . \
        --proof "$checked_in_proof"
else
    echo "::notice:: conformance promotion proof skipped: the draft coverage matrix still has pending requirements"
fi
