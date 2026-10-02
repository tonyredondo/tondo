#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-cbor-tests.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp_dir/$name.log" 2>&1; then
        echo "std.cbor tests: $name unexpectedly passed" >&2; exit 1
    fi
}
for mutation in \
    'status|.status="pending"' \
    'bounds|.limits.max_reference_depth=256' \
    'oracle|.model.production_imports=true' \
    'corpus|.corpus.valid_cases=60' \
    'fuzz|.fuzz.smoke.result="failed"' \
    'native|.promotion.native_aot="verified"' \
    'next|.promotion.next_blocks=["STD-CBOR-TEST-001"]' \
    'parent|.parent_contract="testing/other.json"'; do
    name="${mutation%%|*}"
    jq "${mutation#*|}" testing/stdlib-cbor-test.json > "$tmp_dir/$name.json"
    expect_failure "$name" env TONDO_STDLIB_CBOR_TEST_CONTRACT="$tmp_dir/$name.json" scripts/stdlib-cbor-test-check.sh
done
for mutation in 'duplicate|.valid[1].id=.valid[0].id' 'wire|.valid[0].wire="z0"' \
    'collision|.valid[0].deterministic_error="KeyCollision"' 'missing|.invalid=.invalid[1:]'; do
    name="${mutation%%|*}"
    jq "${mutation#*|}" crates/tondo-reliability/tests/fixtures/cbor-wire.json > "$tmp_dir/$name.json"
    expect_failure "$name" env TONDO_STDLIB_CBOR_TEST_CORPUS="$tmp_dir/$name.json" scripts/stdlib-cbor-test-check.sh
done
scripts/stdlib-cbor-test-check.sh
# The CI runner provides standard utilities, without requiring ripgrep.
mkdir "$tmp_dir/no-rg" "$tmp_dir/no-grep"
for utility in bash jq tail cmp grep; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp_dir/no-rg/$utility"
    if [[ "$utility" != grep ]]; then
        ln -s "$utility_path" "$tmp_dir/no-grep/$utility"
    fi
done
env PATH="$tmp_dir/no-rg" scripts/stdlib-cbor-test-check.sh
expect_failure missing-inspection-tool env PATH="$tmp_dir/no-grep" scripts/stdlib-cbor-test-check.sh
cargo test -q -p tondo-reliability --test cbor_models --locked
cargo test -q -p tondo-stdlib cbor::tests --locked
cargo check -q --manifest-path fuzz/Cargo.toml --bin stdlib_cbor --no-default-features --locked
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
# Ordinary fuzz commands must still include all existing compiler targets.
cargo check -q --manifest-path fuzz/Cargo.toml --bins --locked
echo "std.cbor tests: OK (12 rejected contract mutations, utility availability, model/kernel tests and minimal/full harness builds)"
