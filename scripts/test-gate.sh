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
run_step diagnostic-runtime-contract \
    scripts/diagnostic-runtime-check.sh
run_step diagnostic-runtime-contract-tests \
    scripts/diagnostic-runtime-test.sh
run_step diagnostic-race-contract \
    scripts/diagnostic-race-check.sh
run_step diagnostic-race-contract-tests \
    scripts/diagnostic-race-test.sh
run_step diagnostic-leak-contract \
    scripts/diagnostic-leak-check.sh
run_step diagnostic-leak-contract-tests \
    scripts/diagnostic-leak-test.sh
run_step diagnostic-dump-contract \
    scripts/diagnostic-dump-check.sh
run_step diagnostic-dump-contract-tests \
    scripts/diagnostic-dump-test.sh
run_step diagnostic-test-contract \
    scripts/diagnostic-test-check.sh
run_step diagnostic-test-contract-tests \
    scripts/diagnostic-test-test.sh
run_step diagnostic-ci-contract \
    scripts/diagnostic-ci-check.sh
run_step diagnostic-ci-contract-tests \
    scripts/diagnostic-ci-test.sh
run_step diagnostic-native-contract \
    scripts/native-diagnostics-check.sh
run_step diagnostic-native-contract-tests \
    scripts/native-diagnostics-test.sh
run_step stdlib-async-group-contract \
    scripts/stdlib-async-group-check.sh
run_step stdlib-async-group-contract-tests \
    scripts/stdlib-async-group-test.sh
run_step stdlib-async-group-performance-contract-tests \
    scripts/stdlib-async-group-performance-test.sh
run_step stdlib-async-group-performance \
    scripts/stdlib-async-group-performance.sh
run_step stdlib-async-group-conformance-contract \
    scripts/stdlib-async-group-conformance-check.sh
run_step stdlib-async-group-conformance-contract-tests \
    scripts/stdlib-async-group-conformance-test.sh
run_step stdlib-async-group-conformance \
    scripts/stdlib-async-group-conformance.sh
run_step stdlib-async-group-documentation-contract \
    scripts/stdlib-async-group-doc-check.sh
run_step stdlib-async-group-documentation-contract-tests \
    scripts/stdlib-async-group-doc-test.sh
run_step stdlib-channel-contract \
    scripts/stdlib-channel-check.sh
run_step stdlib-channel-contract-tests \
    scripts/stdlib-channel-test.sh
run_step stdlib-channel-implementation-contract \
    scripts/stdlib-channel-implementation-check.sh
run_step stdlib-channel-implementation-contract-tests \
    scripts/stdlib-channel-implementation-test.sh
run_step stdlib-channel-implementation \
    scripts/stdlib-channel-implementation.sh
run_step stdlib-channel-async-iter-contract \
    scripts/stdlib-channel-async-iter-check.sh
run_step stdlib-channel-async-iter-contract-tests \
    scripts/stdlib-channel-async-iter-test.sh
run_step stdlib-channel-async-iter-implementation \
    scripts/stdlib-channel-async-iter.sh
run_step stdlib-channel-testing-contract \
    scripts/stdlib-channel-test-check.sh
run_step stdlib-channel-testing-contract-tests \
    scripts/stdlib-channel-test-test.sh
run_step stdlib-channel-performance-contract \
    scripts/stdlib-channel-performance-check.sh
run_step stdlib-channel-performance-contract-tests \
    scripts/stdlib-channel-performance-test.sh
run_step stdlib-channel-performance \
    scripts/stdlib-channel-performance.sh
run_step stdlib-channel-conformance-contract \
    scripts/stdlib-channel-conformance-check.sh
run_step stdlib-channel-conformance-contract-tests \
    scripts/stdlib-channel-conformance-test.sh
run_step stdlib-channel-conformance \
    scripts/stdlib-channel-conformance.sh
run_step stdlib-channel-documentation-contract \
    scripts/stdlib-channel-doc-check.sh
run_step stdlib-channel-documentation-contract-tests \
    scripts/stdlib-channel-doc-test.sh
run_step stdlib-sync-contract \
    scripts/stdlib-sync-check.sh
run_step stdlib-sync-contract-tests \
    scripts/stdlib-sync-test.sh
run_step stdlib-sync-performance-contract-tests \
    scripts/stdlib-sync-performance-test.sh
run_step stdlib-sync-performance \
    scripts/stdlib-sync-performance.sh
run_step stdlib-sync-collection-frontend-contract \
    scripts/stdlib-sync-collection-frontend-check.sh
run_step stdlib-sync-collection-frontend-contract-tests \
    scripts/stdlib-sync-collection-frontend-test.sh
run_step stdlib-sync-collection-contract \
    scripts/stdlib-sync-collection-check.sh
run_step stdlib-sync-collection-contract-tests \
    scripts/stdlib-sync-collection-test.sh
run_step stdlib-sync-collection-iter-contract \
    scripts/stdlib-sync-collection-iter-check.sh
run_step stdlib-sync-collection-iter-contract-tests \
    scripts/stdlib-sync-collection-iter-test.sh
run_step stdlib-sync-collection-test-contract \
    scripts/stdlib-sync-collection-test-check.sh
run_step stdlib-sync-collection-test-contract-tests \
    scripts/stdlib-sync-collection-test-test.sh
run_step stdlib-sync-collection-performance-contract \
    scripts/stdlib-sync-collection-performance-check.sh
run_step stdlib-sync-collection-performance-contract-tests \
    scripts/stdlib-sync-collection-performance-test.sh
run_step stdlib-sync-collection-performance \
    scripts/stdlib-sync-collection-performance.sh
run_step stdlib-sync-collection-conformance-contract \
    scripts/stdlib-sync-collection-conformance-check.sh
run_step stdlib-sync-collection-conformance-contract-tests \
    scripts/stdlib-sync-collection-conformance-test.sh
run_step stdlib-sync-collection-conformance \
    scripts/stdlib-sync-collection-conformance.sh
run_step stdlib-sync-conformance-contract \
    scripts/stdlib-sync-conformance-check.sh
run_step stdlib-sync-conformance-contract-tests \
    scripts/stdlib-sync-conformance-test.sh
run_step stdlib-sync-conformance \
    scripts/stdlib-sync-conformance.sh
run_step stdlib-sync-documentation-contract \
    scripts/stdlib-sync-doc-check.sh
run_step stdlib-sync-documentation-contract-tests \
    scripts/stdlib-sync-doc-test.sh
run_step stdlib-executor-contract \
    scripts/stdlib-executor-check.sh
run_step stdlib-executor-contract-tests \
    scripts/stdlib-executor-test.sh
run_step stdlib-executor-implementation-contract \
    scripts/stdlib-executor-implementation-check.sh
run_step stdlib-executor-implementation-contract-tests \
    scripts/stdlib-executor-implementation-test.sh
run_step stdlib-executor-implementation \
    scripts/stdlib-executor-implementation.sh
run_step stdlib-executor-performance-contract \
    scripts/stdlib-executor-performance-check.sh
run_step stdlib-executor-performance-contract-tests \
    scripts/stdlib-executor-performance-test.sh
run_step stdlib-executor-performance \
    scripts/stdlib-executor-performance.sh
run_step stdlib-executor-conformance-contract \
    scripts/stdlib-executor-conformance-check.sh
run_step stdlib-executor-conformance-contract-tests \
    scripts/stdlib-executor-conformance-test.sh
run_step stdlib-executor-conformance \
    scripts/stdlib-executor-conformance.sh
run_step stdlib-executor-documentation-contract \
    scripts/stdlib-executor-doc-check.sh
run_step stdlib-executor-documentation-contract-tests \
    scripts/stdlib-executor-doc-test.sh
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
run_step stdlib-uuid-contract \
    scripts/stdlib-uuid-check.sh
run_step stdlib-uuid-contract-tests \
    scripts/stdlib-uuid-test.sh
run_step stdlib-log-contract \
    scripts/stdlib-log-check.sh
run_step stdlib-log-contract-tests \
    scripts/stdlib-log-test.sh
run_step stdlib-performance-contract \
    scripts/stdlib-performance-check.sh
run_step native-target-descriptor-contract \
    scripts/native-target-descriptor-check.sh
run_step native-target-descriptor-tests \
    scripts/native-target-descriptor-test.sh
run_step native-memory-contract \
    scripts/native-memory-check.sh
run_step native-memory-contract-tests \
    scripts/native-memory-test.sh
run_step native-arc-contract \
    scripts/native-arc-check.sh
run_step native-arc-contract-tests \
    scripts/native-arc-test.sh
run_step native-abi-contract \
    scripts/native-abi-check.sh
run_step native-abi-contract-tests \
    scripts/native-abi-test.sh
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
run_step native-evaluation-contract \
    scripts/native-evaluation-check.sh
run_step native-evaluation-tests \
    scripts/native-evaluation-test.sh
run_step native-evaluation-fast-contract-tests \
    scripts/native-evaluation-fast-test.sh
run_step native-evaluation-runner-contract \
    scripts/native-evaluation-runner-check.sh
run_step native-evaluation-runner-contract-tests \
    scripts/native-evaluation-runner-test.sh
run_step native-std-core-contract \
    scripts/native-std-core-check.sh
run_step native-std-core-contract-tests \
    scripts/native-std-core-test.sh
run_step native-std-hosted-contract \
    scripts/native-std-hosted-check.sh
run_step native-std-hosted-tests \
    scripts/native-std-hosted-test.sh
run_step native-std-contract \
    scripts/native-std-check.sh
run_step native-std-tests \
    scripts/native-std-test.sh
run_step native-link-contract \
    scripts/native-link-check.sh
run_step native-link-tests \
    scripts/native-link-test.sh
run_step native-cli-contract \
    scripts/native-cli-check.sh
run_step native-cli-tests \
    scripts/native-cli-test.sh
run_step native-conf-adapter-contract \
    scripts/native-conf-adapter-check.sh
run_step native-conf-adapter-tests \
    scripts/native-conf-adapter-test.sh
run_step native-conf-language-contract \
    scripts/native-conf-language-check.sh
run_step native-conf-language-tests \
    scripts/native-conf-language-test.sh
run_step native-conf-testing-contract \
    scripts/native-conf-testing-check.sh
run_step native-conf-testing-tests \
    scripts/native-conf-testing-test.sh
run_step native-conf-stdlib-contract \
    scripts/native-conf-stdlib-check.sh
run_step native-conf-stdlib-tests \
    scripts/native-conf-stdlib-test.sh
run_step native-conf-contract \
    scripts/native-conf-check.sh
run_step native-conf-tests \
    scripts/native-conf-test.sh
run_step native-diff-contract \
    scripts/native-diff-check.sh
run_step native-diff-tests \
    scripts/native-diff-test.sh
run_step native-target-registry-contract \
    scripts/native-target-check.sh
run_step native-target-registry-tests \
    scripts/native-target-test.sh
run_step native-target-arm64-registry-contract \
    scripts/native-target-aarch64-check.sh
run_step native-target-arm64-registry-tests \
    scripts/native-target-aarch64-contract-test.sh
run_step native-release-contract \
    scripts/native-rel-check.sh
run_step native-release-tests \
    scripts/native-rel-test.sh
run_step native-selection-contract \
    scripts/native-selection-check.sh
run_step native-selection-contract-tests \
    scripts/native-selection-test.sh
run_step native-aot-scope-contract \
    scripts/native-aot-scope-check.sh
run_step native-aot-scope-contract-tests \
    scripts/native-aot-scope-test.sh
run_step native-aot-lowering-contract \
    scripts/native-aot-lowering-check.sh
run_step native-aot-lowering-contract-tests \
    scripts/native-aot-lowering-test.sh
run_step native-aot-binary-contract \
    scripts/native-aot-binary-check.sh
run_step native-aot-binary-contract-tests \
    scripts/native-aot-binary-test.sh
run_step native-aot-memory-contract \
    scripts/native-aot-memory-check.sh
run_step native-aot-memory-contract-tests \
    scripts/native-aot-memory-test.sh
run_step native-aot-quality-contract \
    scripts/native-aot-quality-check.sh
run_step native-aot-quality-contract-tests \
    scripts/native-aot-quality-test.sh
run_step native-aot-performance-contract \
    scripts/native-aot-performance-check.sh
run_step native-aot-performance-contract-tests \
    scripts/native-aot-performance-test.sh
run_step native-select-contract \
    scripts/native-select-check.sh
run_step native-select-contract-tests \
    scripts/native-select-test.sh
run_step native-lowering-debug-contract \
    scripts/native-lowering-debug-check.sh
run_step native-lowering-debug-contract-tests \
    scripts/native-lowering-debug-test.sh
run_step native-thread-contract \
    scripts/native-thread-check.sh
run_step native-thread-contract-tests \
    scripts/native-thread-test.sh
run_step native-lowering-contract \
    scripts/native-lowering-check.sh
run_step native-lowering-contract-tests \
    scripts/native-lowering-test.sh
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
