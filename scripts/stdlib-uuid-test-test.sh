#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-uuid-tests.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
negative_count=0
expect_failure() {
    if "$@" > "$tmp_dir/rejected.log" 2>&1; then
        echo "std.uuid tests: invalid record unexpectedly passed" >&2; exit 1
    fi
    negative_count=$((negative_count + 1))
}
for transition in ready verified; do
    jq --arg status "$transition" '
      .status=$status
      | .quality_gate=(if $status == "verified" then "verified-80-percent-per-scope" else "pending-80-percent-per-scope" end)
      | .promotion.test_boundary_promoted=($status == "verified")
    ' testing/stdlib-uuid-test.json > "$tmp_dir/base.json"
    jq --slurpfile child "$tmp_dir/base.json" '
      del(.measurement, .conformance)
      | .implementation.required_follow_ups=["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
      | .model.status=$child[0].status | .model.quality_gate=$child[0].quality_gate
      | .promotion.next_blocks=(if $child[0].status == "verified" then ["STD-UUID-PERF-001"] else ["STD-UUID-TEST-001"] end)
    ' testing/stdlib-uuid.json > "$tmp_dir/parent.json"
    env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/parent.json" TONDO_STDLIB_UUID_TEST_CONTRACT="$tmp_dir/base.json" scripts/stdlib-uuid-test-check.sh
    while IFS= read -r mutation; do
        jq "$mutation" "$tmp_dir/base.json" > "$tmp_dir/invalid.json"
        expect_failure env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/parent.json" TONDO_STDLIB_UUID_TEST_CONTRACT="$tmp_dir/invalid.json" scripts/stdlib-uuid-test-check.sh
    done <<'MUTATIONS'
.status="pending"
.quality_gate="passed"
.quality_gate=(if .quality_gate == "verified-80-percent-per-scope" then "pending-80-percent-per-scope" else "verified-80-percent-per-scope" end)
.promotion.test_boundary_promoted |= not
.target="native-AOT"
.limits.max_reference_name_bytes=97
.limits.max_reference_provider_rows=256
.limits.model_seed_count=128
.model.production_imports=true
.model.oracle="kernel-as-oracle"
.model.name_digest_domain="all-production-names"
.model.provider_domain="complete-OS-provider"
.model.outside_domain="production-rejection"
.corpus.valid_cases=16
.test.integration_tests=10
.test.kernel_tests=0
.test.host_tests=0
.test.production_vm_transcript="model-only"
.fuzz.status="ready"
.fuzz.smoke.result="pending"
.fuzz.smoke.runs=1
.fuzz.smoke.toolchain="nightly"
.fuzz.input_limit_bytes=8192
.fuzz.step_limit=513
.fuzz.minimal_dependency_graph="full-compiler"
.promotion.public_api_promoted=true
.promotion.production_vm_registration="native"
.promotion.native_abi="verified"
.promotion.native_aot="verified"
.promotion.simd="verified"
.promotion.performance="verified"
.promotion.next_blocks=["STD-UUID-DOC-001"]
MUTATIONS
    for mutation in '.model.status="published"' '.model.quality_gate="passed"' \
        '.model.selected_route="native"' '.host.status="ready-production-hosted"' \
        '.promotion.next_blocks=["STD-UUID-DOC-001"]'; do
        jq "$mutation" "$tmp_dir/parent.json" > "$tmp_dir/invalid-parent.json"
        expect_failure env TONDO_STDLIB_UUID_CONTRACT="$tmp_dir/invalid-parent.json" TONDO_STDLIB_UUID_TEST_CONTRACT="$tmp_dir/base.json" scripts/stdlib-uuid-test-check.sh
    done
done
for mutation in '.valid[1].id=.valid[0].id' '.valid[0].operation="ambient-rng"' \
    '.valid[0].value="0000"' '.valid[0].version=16' '.valid[0].variant="Other"' \
    '.valid[2].bytes_hex="zz"' '.invalid[0].error="OutsideDomain"' \
    '.invalid[0].offset=-1' '.invalid[0] |= del(.offset)' '.invalid |= .[1:]'; do
    jq "$mutation" crates/tondo-reliability/tests/fixtures/uuid-cases.json > "$tmp_dir/invalid-corpus.json"
    expect_failure env TONDO_STDLIB_UUID_TEST_CORPUS="$tmp_dir/invalid-corpus.json" scripts/stdlib-uuid-test-check.sh
done
scripts/stdlib-uuid-test-check.sh
mkdir "$tmp_dir/no-rg" "$tmp_dir/no-grep"
for utility in bash jq tail cmp grep dirname; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp_dir/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp_dir/no-grep/$utility"
done
env PATH="$tmp_dir/no-rg" scripts/stdlib-uuid-test-check.sh
expect_failure env PATH="$tmp_dir/no-grep" scripts/stdlib-uuid-test-check.sh
cargo test -q -p tondo-reliability --test uuid_models --locked
cargo test -q -p tondo-stdlib --locked --lib uuid::tests
cargo check -q --manifest-path fuzz/Cargo.toml --bin stdlib_uuid --no-default-features --locked
host="$(rustc -vV | sed -n 's/^host: //p')"
cargo metadata --manifest-path fuzz/Cargo.toml --format-version 1 --locked \
    --no-default-features --filter-platform "$host" > "$tmp_dir/minimal.json"
jq -e '
    [.resolve.nodes[].id] as $active
    | [.packages[] | select(.id as $id | $active | index($id)) | .name] as $names
    | ($names | index("tondo-stdlib")) != null
      and all($names[]; . != "tondo-compiler" and . != "tondo-vm"
          and . != "tondo-conformance" and . != "tondo-reliability")
' "$tmp_dir/minimal.json" >/dev/null
cargo check -q --manifest-path fuzz/Cargo.toml --bins --locked
echo "std.uuid tests: OK ($negative_count invalid records; both progression states; 11 model/VM and 18 kernel tests; minimal/full fuzz builds)"
