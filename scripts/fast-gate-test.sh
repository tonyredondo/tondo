#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

for helper in scripts/fast-coverage-check.sh scripts/fast-gate.sh \
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

impacted="$(TONDO_FAST_CHANGED_FILES=$'crates/tondo-stdlib/src/lib.rs\nscripts/fast-gate.sh' \
    TONDO_FAST_GATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tondo-fast-gate-test.XXXXXX")" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$impacted" "scope=impacted"
assert_contains "$impacted" "check-tondo-stdlib"

shared="$(TONDO_FAST_CHANGED_FILES='crates/tondo-compiler/src/hir/check.rs' \
    TONDO_FAST_GATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/tondo-fast-gate-test.XXXXXX")" \
    bash scripts/fast-gate.sh --dry-run)"
assert_contains "$shared" "scope=shared-frontier"
assert_contains "$shared" "full-test-gate"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-fast-gate-test.XXXXXX")"
trap 'rm -rf -- "$tmp_dir"' EXIT
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
