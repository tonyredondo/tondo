#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-regex-tests.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    local name="$1"; shift
    if "$@" > "$tmp_dir/$name.log" 2>&1; then
        echo "std.regex tests: $name unexpectedly passed" >&2; exit 1
    fi
}
for transition in ready verified; do
    if [[ "$transition" == ready ]]; then
        jq '.status="ready" | .quality_gate="pending-80-percent-per-scope" | .promotion.test_boundary_promoted=false' testing/stdlib-regex-test.json > "$tmp_dir/base.json"
    else
        jq '.status="verified" | .quality_gate="verified-80-percent-per-scope" | .promotion.test_boundary_promoted=true' testing/stdlib-regex-test.json > "$tmp_dir/base.json"
    fi
    TONDO_STDLIB_REGEX_TEST_CONTRACT="$tmp_dir/base.json" scripts/stdlib-regex-test-check.sh
    for mutation in \
        'status|.status="pending"' 'quality|.quality_gate="passed"' \
        'bounds|.limits.max_reference_depth=256' 'oracle|.model.production_imports=true' \
        'unicode|.model.unicode_oracle="complete-unicode-tables"' \
        'corpus|.corpus.valid_cases=40' 'fuzz|.fuzz.smoke.result="failed"' \
        'tests|.test.integration_tests=9' 'native|.promotion.native_aot="verified"' \
        'next|.promotion.next_blocks=["STD-REGEX-DOC-001"]'; do
        name="${mutation%%|*}"
        jq "${mutation#*|}" "$tmp_dir/base.json" > "$tmp_dir/$name.json"
        expect_failure "$name" env TONDO_STDLIB_REGEX_TEST_CONTRACT="$tmp_dir/$name.json" scripts/stdlib-regex-test-check.sh
    done
done
for mutation in 'duplicate|.valid[1].id=.valid[0].id' 'span|.valid[0].first=[[2,1]]' \
    'unicode-version|.unicode="15.1.0"' 'domain|.valid[0].model=false' \
    'missing|.invalid=.invalid[1:]'; do
    name="${mutation%%|*}"
    jq "${mutation#*|}" crates/tondo-reliability/tests/fixtures/regex-cases.json > "$tmp_dir/$name.json"
    expect_failure "$name" env TONDO_STDLIB_REGEX_TEST_CORPUS="$tmp_dir/$name.json" scripts/stdlib-regex-test-check.sh
done
scripts/stdlib-regex-test-check.sh
mkdir "$tmp_dir/no-rg" "$tmp_dir/no-grep"
for utility in bash jq tail cmp grep; do
    utility_path="$(command -v "$utility")"
    ln -s "$utility_path" "$tmp_dir/no-rg/$utility"
    [[ "$utility" == grep ]] || ln -s "$utility_path" "$tmp_dir/no-grep/$utility"
done
env PATH="$tmp_dir/no-rg" scripts/stdlib-regex-test-check.sh
expect_failure missing-inspection-tool env PATH="$tmp_dir/no-grep" scripts/stdlib-regex-test-check.sh
cargo test -q -p tondo-reliability --test regex_models --locked
cargo test -q -p tondo-stdlib --test regex_kernel --locked
cargo check -q --manifest-path fuzz/Cargo.toml --bin stdlib_regex --no-default-features --locked
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
echo "std.regex tests: OK (25 rejected contract mutations, utility availability, models/kernel and minimal/full fuzz builds)"
