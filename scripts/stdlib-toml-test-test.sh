#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-toml-test.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.toml tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "contract-locked"' testing/stdlib-toml-test.json > "$tmp_dir/status.json"
expect_failure status env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/status.json" scripts/stdlib-toml-test-check.sh

jq '.limits.max_fuzz_steps = 1024' testing/stdlib-toml-test.json > "$tmp_dir/steps.json"
expect_failure steps env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/steps.json" scripts/stdlib-toml-test-check.sh

jq '.fuzz.smoke.result = "failed"' testing/stdlib-toml-test.json > "$tmp_dir/fuzz.json"
expect_failure fuzz-result env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/fuzz.json" scripts/stdlib-toml-test-check.sh

jq '.sanitization.native_aot = "verified"' testing/stdlib-toml-test.json > "$tmp_dir/aot.json"
expect_failure aot-claim env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/aot.json" scripts/stdlib-toml-test-check.sh

jq '.promotion.next_blocks = ["STD-TOML-TEST-001"]' testing/stdlib-toml-test.json > "$tmp_dir/promotion.json"
expect_failure promotion env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/promotion.json" scripts/stdlib-toml-test-check.sh

jq '.model.sources = .model.sources[0:1]' testing/stdlib-toml-test.json > "$tmp_dir/model.json"
expect_failure missing-model env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/model.json" scripts/stdlib-toml-test-check.sh

jq '.parent_contract = "testing/other-test.json"' testing/stdlib-toml-test.json > "$tmp_dir/parent.json"
expect_failure parent-link env TONDO_STDLIB_TOML_TEST_CONTRACT="$tmp_dir/parent.json" scripts/stdlib-toml-test-check.sh

scripts/stdlib-toml-test-check.sh >/dev/null

target_dir="${CARGO_TARGET_DIR:-$root/target}"
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-reliability --test toml_models --locked
CARGO_TARGET_DIR="$target_dir" cargo test -q -p tondo-stdlib toml::tests --locked
CARGO_TARGET_DIR="$target_dir" cargo check -q --manifest-path fuzz/Cargo.toml \
    --bin stdlib_toml --no-default-features --locked

# Inspect Cargo's actual host dependency closure, including transitive edges.
# A direct dependency check alone would miss a compiler pulled in by a model.
host="$(rustc -vV | sed -n 's/^host: //p')"
cargo metadata --manifest-path fuzz/Cargo.toml --format-version 1 --locked \
    --no-default-features --filter-platform "$host" > "$tmp_dir/minimal.json"
jq -e '
    [.resolve.nodes[].id] as $active
    | [.packages[] | select(.id as $id | $active | index($id)) | .name] as $names
    | ($names | index("tondo-stdlib")) != null
      and all($names[];
          . != "tondo-compiler" and . != "tondo-vm"
          and . != "tondo-conformance" and . != "tondo-reliability")
' "$tmp_dir/minimal.json" >/dev/null || {
    echo "std.toml tests: minimal harness pulls in compiler or runtime dependencies" >&2
    exit 1
}

# All existing targets must still build with the default feature set.
cargo metadata --manifest-path fuzz/Cargo.toml --format-version 1 --locked \
    --filter-platform "$host" > "$tmp_dir/default.json"
jq -e '
    .resolve.root as $root
    | (.resolve.nodes[] | select(.id == $root) | .features) as $enabled
    | all(.packages[] | select(.id == $root) | .targets[];
        all(."required-features"[]?; . as $feature | $enabled | index($feature)))
' "$tmp_dir/default.json" >/dev/null || {
    echo "std.toml tests: default features skip a declared fuzz target" >&2
    exit 1
}
CARGO_TARGET_DIR="$target_dir" cargo check -q --manifest-path fuzz/Cargo.toml --bins --locked

echo "std.toml tests: OK (negative testing contract; independent model; hosted regressions; minimal harness; all default fuzz targets)"
