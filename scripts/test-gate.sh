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
echo "::group::layer-evidence-before"
cargo run -p tondo-reliability --locked -- quality provenance --root . \
    > "$evidence/layer-evidence-before.json"
cat "$evidence/layer-evidence-before.json"
echo "::endgroup::"
run_step test cargo test --workspace --all-targets --locked
run_step layer-evidence-attest \
    cargo run -p tondo-reliability --locked -- layer-evidence attest \
    --root . \
    --test-log "$logs/test.log" \
    --before "$evidence/layer-evidence-before.json" \
    --output "$evidence/layer-evidence.json"
run_step rustdoc env RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps --locked
run_step conformance-build \
    cargo build -p tondo-conformance -p tondo-reference-adapter --bins --locked
run_step doc-test \
    scripts/doc-test.sh
run_step doc-test-conformance-tests \
    scripts/doc-test-conformance-test.sh
run_step reliability \
    cargo run -p tondo-reliability --locked -- check --root .
run_step tracker-lint \
    cargo run -p tondo-reliability --locked -- tracker lint --root .
run_step mutation-infrastructure-check-tests \
    scripts/mutation-infrastructure-check-test.sh
run_step performance-contract \
    scripts/performance-check.sh
run_step performance-tests \
    scripts/performance-test.sh
run_step async-select-performance-contract-tests \
    scripts/async-select-performance-test.sh
run_step async-select-performance \
    scripts/async-select-performance.sh
run_step diagnostic-contract \
    scripts/diagnostic-contract-check.sh
run_step diagnostic-contract-tests \
    scripts/diagnostic-contract-test.sh
run_step stdlib-async-group-contract \
    scripts/stdlib-async-group-check.sh
run_step stdlib-async-group-contract-tests \
    scripts/stdlib-async-group-test.sh
run_step stdlib-channel-contract \
    scripts/stdlib-channel-check.sh
run_step stdlib-channel-contract-tests \
    scripts/stdlib-channel-test.sh
run_step stdlib-sync-contract \
    scripts/stdlib-sync-check.sh
run_step stdlib-sync-contract-tests \
    scripts/stdlib-sync-test.sh
run_step stdlib-executor-contract \
    scripts/stdlib-executor-check.sh
run_step stdlib-executor-contract-tests \
    scripts/stdlib-executor-test.sh
run_step stdlib-net-contract \
    scripts/stdlib-net-check.sh
run_step stdlib-net-contract-tests \
    scripts/stdlib-net-test.sh
run_step stdlib-civil-time-contract \
    scripts/stdlib-civil-time-check.sh
run_step stdlib-civil-time-contract-tests \
    scripts/stdlib-civil-time-test.sh
run_step stdlib-encoding-contract \
    scripts/stdlib-encoding-check.sh
run_step stdlib-encoding-contract-tests \
    scripts/stdlib-encoding-test.sh
run_step stdlib-yaml-contract \
    scripts/stdlib-yaml-check.sh
run_step stdlib-yaml-contract-tests \
    scripts/stdlib-yaml-test.sh
run_step stdlib-toml-contract \
    scripts/stdlib-toml-check.sh
run_step stdlib-toml-contract-tests \
    scripts/stdlib-toml-test.sh
run_step stdlib-cbor-contract \
    scripts/stdlib-cbor-check.sh
run_step stdlib-cbor-contract-tests \
    scripts/stdlib-cbor-test.sh
run_step stdlib-regex-contract \
    scripts/stdlib-regex-check.sh
run_step stdlib-regex-contract-tests \
    scripts/stdlib-regex-test.sh
run_step stdlib-performance-contract \
    scripts/stdlib-performance-check.sh
run_step native-target-descriptor-contract \
    scripts/native-target-descriptor-check.sh
run_step native-target-descriptor-tests \
    scripts/native-target-descriptor-test.sh
run_step native-artifact-contract \
    scripts/native-artifact-check.sh
run_step native-artifact-tests \
    scripts/native-artifact-test.sh
run_step native-link-plan-contract \
    scripts/native-link-plan-check.sh
run_step native-link-plan-tests \
    scripts/native-link-plan-test.sh
run_step native-publish-contract \
    scripts/native-publish-check.sh
run_step native-publish-tests \
    scripts/native-publish-test.sh
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
run_step stdlib-async-contract \
    scripts/stdlib-async-check.sh
run_step stdlib-owner-evidence \
    scripts/stdlib-owner-evidence-check.sh
run_step stdlib-fuzz-contract \
    scripts/stdlib-fuzz-check.sh
run_step stdlib-fuzz-tests \
    scripts/stdlib-fuzz-test.sh
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
run_step stdlib-async-tests \
    scripts/stdlib-async-test.sh
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
run_step stdlib-implementation-coordination \
    scripts/stdlib-implementation-coordination-check.sh
run_step stdlib-implementation-coordination-tests \
    scripts/stdlib-implementation-coordination-test.sh
run_step stdlib-hosted-implementation-coordination \
    scripts/stdlib-hosted-implementation-coordination-check.sh
run_step stdlib-hosted-implementation-coordination-tests \
    scripts/stdlib-hosted-implementation-coordination-test.sh
run_step stdlib-public-api-audit \
    scripts/stdlib-public-api-audit.sh --check
run_step stdlib-public-api-audit-tests \
    scripts/stdlib-public-api-audit-test.sh
run_step stdlib-normative-matrix \
    scripts/stdlib-matrix-check.sh
run_step stdlib-normative-matrix-tests \
    scripts/stdlib-matrix-test.sh
run_step stdlib-conformance \
    scripts/stdlib-conformance.sh
run_step stdlib-conformance-tests \
    scripts/stdlib-conformance-test.sh
run_step stdlib-conformance-check \
    scripts/stdlib-conformance-check.sh
run_step stdlib-distribution-contract \
    scripts/stdlib-distribution-check.sh
run_step stdlib-distribution-tests \
    scripts/stdlib-distribution-test.sh
run_step stdlib-distribution \
    scripts/stdlib-distribution.sh
run_step stdlib-codec-conformance \
    scripts/stdlib-codec-conformance.sh
run_step stdlib-conformance-coordination \
    scripts/stdlib-conformance-coordination-check.sh
run_step stdlib-conformance-coordination-tests \
    scripts/stdlib-conformance-coordination-test.sh
run_step stdlib-documentation \
    scripts/stdlib-documentation-check.sh
run_step stdlib-documentation-tests \
    scripts/stdlib-documentation-test.sh
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
    --evidence "$evidence/layer-evidence.json" \
    --output "$evidence/conformance-result.json"
run_step async-select-conformance \
    scripts/async-select-conformance.sh
run_step async-select-conformance-contract-tests \
    scripts/async-select-conformance-test.sh
run_step stdlib-s1a-seal \
    scripts/stdlib-s1a-seal.sh
run_step stdlib-s1a-seal-tests \
    scripts/stdlib-s1a-seal-test.sh
