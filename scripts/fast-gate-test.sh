#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for helper in scripts/documentation-gate.sh scripts/fast-coverage-check.sh scripts/fast-gate.sh \
    scripts/fast-gate-test.sh; do
    [[ -x "$helper" ]] || {
        echo "fast gate test: helper is not executable: $helper" >&2
        exit 1
    }
done

assert_contains() {
    local haystack="$1" needle="$2"
    grep -Fq -- "$needle" <<< "$haystack" || {
        echo "fast gate test: expected '$needle'" >&2
        exit 1
    }
}

assert_not_contains() {
    local haystack="$1" needle="$2"
    if grep -Fq -- "$needle" <<< "$haystack"; then
        echo "fast gate test: did not expect '$needle'" >&2
        exit 1
    fi
}

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-fast-gate-test.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT

documentation="$(TONDO_FAST_CHANGED_FILES=$'TONDO_LANGUAGE_SPEC.md\ntesting/coverage-matrix.json\nconformance/0.1/cases/documentation/language-spec.expect.json' \
    TONDO_FAST_GATE_DIR="$tmp_dir/documentation" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$documentation" "scope=documentation"
assert_contains "$documentation" "documentation-gate"
assert_not_contains "$documentation" "full-test-gate"
assert_not_contains "$documentation" "changed-line-coverage"
assert_not_contains "$documentation" "diff-mutation"

test_only="$(TONDO_FAST_CHANGED_FILES='crates/tondo-reliability/tests/contract.rs' \
    TONDO_FAST_GATE_DIR="$tmp_dir/test-only" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$test_only" "scope=impacted"
assert_contains "$test_only" "test-tondo-reliability"
assert_not_contains "$test_only" "changed-line-coverage"
assert_not_contains "$test_only" "diff-mutation"

impacted="$(TONDO_FAST_CHANGED_FILES=$'crates/tondo-stdlib/src/lib.rs\nscripts/fast-gate.sh' \
    TONDO_FAST_GATE_DIR="$tmp_dir/impacted" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$impacted" "scope=impacted"
assert_contains "$impacted" "check-tondo-stdlib"
assert_contains "$impacted" "changed-line-coverage"
assert_contains "$impacted" "diff-mutation"

shared="$(TONDO_FAST_CHANGED_FILES='crates/tondo-compiler/src/hir/check.rs' \
    TONDO_FAST_GATE_DIR="$tmp_dir/shared" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$shared" "scope=shared-frontier"
assert_contains "$shared" "full-test-gate"

shared_monolithic="$(TONDO_FAST_CHANGED_FILES='crates/tondo-compiler/src/mir.rs' \
    TONDO_FAST_GATE_DIR="$tmp_dir/shared-monolithic" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$shared_monolithic" "scope=shared-frontier"
assert_contains "$shared_monolithic" "full-test-gate"

# A contract or checker without an explicit smaller owner must still be
# validated. Formatter-only success cannot prove these inputs.
for path in testing/stdlib-yaml-performance.json scripts/stdlib-yaml-performance-check.sh \
    fuzz/fuzz_targets/stdlib_toml.rs acceptance/projects/testing/src/main.to \
    scripts/native-arc-check.sh scripts/native-memory-test.sh scripts/documentation-gate.sh \
    .github/workflows/test.yml; do
    unmapped="$(TONDO_FAST_CHANGED_FILES="$path" \
        TONDO_FAST_GATE_DIR="$tmp_dir/unmapped" \
        bash scripts/fast-gate.sh --dry-run)"
    assert_contains "$unmapped" "scope=shared-frontier"
    assert_contains "$unmapped" "full-test-gate"
done

cat > "$tmp_dir/deleted.diff" <<'EOF'
diff --git a/crates/tondo-compiler/src/hir/check.rs b/crates/tondo-compiler/src/hir/check.rs
deleted file mode 100644
--- a/crates/tondo-compiler/src/hir/check.rs
+++ /dev/null
@@ -1 +0,0 @@
-// A removed frontend input remains part of the impact set.
EOF
deleted="$(TONDO_FAST_GATE_DIR="$tmp_dir/deleted" \
    bash scripts/fast-gate.sh --diff "$tmp_dir/deleted.diff" --dry-run)"
assert_contains "$deleted" "scope=shared-frontier"
assert_contains "$deleted" "full-test-gate"

# Metadata-only and quoted/binary patches cannot disappear behind a nearby
# documentary change. Their smallest safe fallback is the complete gate.
for metadata in \
    $'old mode 100644\nnew mode 100755' \
    $'similarity index 100%\nrename from crates/tondo-compiler/src/hir/check.rs\nrename to docs/moved.md' \
    $'GIT binary patch\nliteral 0\nHcmV?d00001' \
    $'--- "a/crates/tondo-compiler/src/quoted path.rs"\n+++ "b/crates/tondo-compiler/src/quoted path.rs"'; do
    cat > "$tmp_dir/metadata.diff" <<'EOF'
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-old
+new
diff --git a/crates/tondo-compiler/src/hir/check.rs b/crates/tondo-compiler/src/hir/check.rs
EOF
    printf '%s\n' "$metadata" >> "$tmp_dir/metadata.diff"
    classified="$(TONDO_FAST_GATE_DIR="$tmp_dir/metadata" \
        bash scripts/fast-gate.sh --diff "$tmp_dir/metadata.diff" --dry-run)"
    assert_contains "$classified" "scope=shared-frontier"
    assert_contains "$classified" "full-test-gate"
done

evaluation="$(TONDO_FAST_CHANGED_FILES=$'tools/native-evaluation/src/main.rs\ntesting/native-evaluation-runner.json' \
    TONDO_FAST_GATE_DIR="$tmp_dir/evaluation" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$evaluation" "scope=evaluation"
assert_contains "$evaluation" "native-evaluation-fast-contract"
assert_contains "$evaluation" "native-evaluation-runner-contract"
assert_contains "$evaluation" "native-evaluation-runner-tests"
assert_contains "$evaluation" "native-aot-lowering-contract"
assert_contains "$evaluation" "native-aot-lowering-tests"
assert_contains "$evaluation" "native-aot-binary-contract"
assert_contains "$evaluation" "native-aot-binary-tests"
assert_contains "$evaluation" "native-aot-memory-contract"
assert_contains "$evaluation" "native-aot-memory-tests"
assert_contains "$evaluation" "native-aot-performance-contract"
assert_contains "$evaluation" "native-aot-performance-tests"
assert_contains "$evaluation" "native-evaluation-adapter-check"
assert_not_contains "$evaluation" "full-test-gate"
assert_not_contains "$evaluation" "changed-line-coverage"
assert_not_contains "$evaluation" "diff-mutation"

# Execute the workflow's real prerequisite step against the selected plans.
# A contract-only full gate needs fuzz even when no .rs path changed.
awk '
    /- name: Detect fast gate prerequisites/ { selected = 1; next }
    selected && /^        run: \|$/ { body = 1; next }
    body && /^          / { print substr($0, 11); next }
    body { exit }
' .github/workflows/test.yml > "$tmp_dir/prerequisites.sh"
[[ -s "$tmp_dir/prerequisites.sh" ]] || {
    echo "fast gate test: missing workflow prerequisite command" >&2
    exit 1
}
while IFS=' ' read -r path quality fuzz; do
    : > "$tmp_dir/github-output"
    TONDO_FAST_CHANGED_FILES="$path" TONDO_FAST_GATE_DIR="$tmp_dir/prerequisites" \
        GITHUB_OUTPUT="$tmp_dir/github-output" bash "$tmp_dir/prerequisites.sh" >/dev/null
    prerequisites="$(cat "$tmp_dir/github-output")"
    assert_contains "$prerequisites" "quality=$quality"
    assert_contains "$prerequisites" "fuzz=$fuzz"
done <<'EOF'
testing/stdlib-yaml-performance.json false true
crates/tondo-compiler/src/hir/check.rs false true
crates/tondo-stdlib/src/lib.rs true false
crates/tondo-cli/tests/cli.rs false false
README.md false false
EOF

diff_fixture="$tmp_dir/fixture.diff"
coverage_fixture="$tmp_dir/coverage.json"
printf '%s\n' \
    '--- /dev/null' \
    '+++ b/crates/tondo-cli/src/main.rs' \
    '@@ -0,0 +1 @@' \
    '+fn fixture_line() {}' > "$diff_fixture"
printf '{"data":[{"files":[{"filename":"%s","segments":[[1,1,0,true,true,false]]}]}]}\n' \
    "$root/crates/tondo-cli/src/main.rs" > "$coverage_fixture"
if bash scripts/fast-coverage-check.sh "$diff_fixture" "$coverage_fixture" "$root" >/dev/null 2>&1; then
    echo "fast gate test: uncovered fixture unexpectedly passed" >&2
    exit 1
fi
sed 's/\[1,1,0/\[1,1,1/' "$coverage_fixture" > "$coverage_fixture.covered"
bash scripts/fast-coverage-check.sh "$diff_fixture" "$coverage_fixture.covered" "$root" >/dev/null

echo "fast gate tests: OK"
