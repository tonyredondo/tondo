#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-testing-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.testing owner tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owner = "std.invalid"' testing/stdlib-testing.json \
    > "$tmp_dir/invalid-owner.json"
expect_failure invalid-owner env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/invalid-owner.json" \
    scripts/stdlib-testing-check.sh

jq '.test_matrix = []' testing/stdlib-testing.json \
    > "$tmp_dir/missing-test-matrix.json"
expect_failure missing-test-matrix env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/missing-test-matrix.json" \
    scripts/stdlib-testing-check.sh

jq '.base.test_only = false' testing/stdlib-testing.json \
    > "$tmp_dir/not-test-only.json"
expect_failure not-test-only env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/not-test-only.json" \
    scripts/stdlib-testing-check.sh

jq '.generation.algorithm = "host-random"' testing/stdlib-testing.json \
    > "$tmp_dir/host-random.json"
expect_failure host-random env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/host-random.json" \
    scripts/stdlib-testing-check.sh

jq '.capabilities.temporary = []' testing/stdlib-testing.json \
    > "$tmp_dir/missing-temporary-capability.json"
expect_failure missing-temporary-capability env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/missing-temporary-capability.json" \
    scripts/stdlib-testing-check.sh

jq '.limits = []' testing/stdlib-testing.json \
    > "$tmp_dir/missing-limits.json"
expect_failure missing-limits env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/missing-limits.json" \
    scripts/stdlib-testing-check.sh

jq '.promotion.next_coordination = "STD-TEST-001"' testing/stdlib-testing.json \
    > "$tmp_dir/stale-coordination.json"
expect_failure stale-coordination env TONDO_STDLIB_TESTING_CONTRACT="$tmp_dir/stale-coordination.json" \
    scripts/stdlib-testing-check.sh

for marker in \
    'assertEqual' \
    'TextDiff' \
    'FloatTolerance' \
    'Option' \
    'Result' \
    'TempDirectory' \
    'Generator.forCase' \
    'Shrink' \
    'P0007' \
    'source sets'; do
    grep -Fqi "$marker" docs/contracts/stdlib-testing.md
done

grep -Fq '"runner-owned"' testing/stdlib-testing.json

for marker in \
    'std.testing' \
    'failNow' \
    'withVirtualTime' \
    'VirtualTime'; do
    grep -Fqi "$marker" TONDO_TESTING_SPEC.md
done

for symbol in \
    'pub struct TextDiff' \
    'pub struct FloatTolerance' \
    'pub struct Generator' \
    'pub trait Shrink' \
    'pub fn diff_text' \
    'pub fn shrink' \
    'pub fn next_int' \
    'pub fn next_bytes' \
    'pub fn next_text'; do
    grep -Fq "$symbol" crates/tondo-stdlib/src/testing.rs
done

for symbol in \
    'pub fn run_with_shrink' \
    'fn run_cases'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/test_generation.rs
done

for symbol in \
    'fn run_leaf(' \
    'fn run_cleanup(' \
    'retry_group'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/test_runtime.rs
done

for symbol in \
    'testing_temp_directory_is_prefix_validated_and_cleanup_is_bounded' \
    'testing_generator_is_replayable_and_returns_typed_bounds_errors' \
    'testing_shrink_is_deterministic_bounded_and_atomic' \
    'testing_host_records_typed_evidence_in_the_installed_envelope' \
    'testing_host_assertions_cover_structural_values_and_terminal_failures' \
    'testing_host_virtual_time_is_lexical_sequential_and_reports_advances' \
    'testing_host_virtual_settle_rejects_external_wait_without_sleeping'; do
    grep -Fq "$symbol" crates/tondo-compiler/src/process_host.rs
done

for marker in \
    'import std.testing' \
    'testing.assertTextEqual' \
    'testing.Generator.new' \
    'testing.withVirtualTime'; do
    grep -Fq "$marker" crates/tondo-compiler/src/driver.rs
done

for test_name in \
    'acceptance_project_is_relocatable_and_reports_canonical_observations' \
    'acceptance_control_project_exposes_fail_now_and_skip_without_visible_context' \
    'acceptance_control_project_publishes_source_attachments_and_snapshots' \
    'testing_module_is_not_available_to_production_entries' \
    'suite_setup_failure_blocks_only_its_subtree_and_reports_the_suite' \
    'acceptance_project_exercises_public_selection_and_sharding_contracts' \
    'acceptance_project_publishes_equivalent_json_and_junit_results' \
    'acceptance_project_dogfoods_repeat_with_fresh_attempts' \
    'acceptance_project_dogfoods_an_isolated_deterministic_retry'; do
    grep -Fq "$test_name" crates/tondo-cli/tests/acceptance_projects.rs
done

for fixture in \
    acceptance/projects/testing-acceptance/tests/service.to \
    acceptance/projects/testing-control/tests/control.to; do
    [[ -f "$fixture" ]] || exit 1
done

jq -e '
  ([.rows[] | select(.owner == "std.testing")] | length) == 25
  and any(.rows[] | select(.owner == "std.testing"); .symbol == "std.testing.assertEqual")
' testing/stdlib-public-api.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.testing"
    and .runtime.kind == "host"
    and (.runtime.paths | length) > 0)
' testing/stdlib-public-api-config.json >/dev/null

jq -e '
  any(.leaves[]; .id == "STD-A-TESTING-EVIDENCE-001" and .owners == ["std.testing"])
  and any(.owners[]; .id == "std.testing"
    and .cells.SPEC.status == "verified"
    and .cells.IMPL.status == "verified"
    and .cells.HOST.status == "verified"
    and .cells.MODEL.status == "verified"
    and .cells.TEST.status == "verified"
    and .cells.FUZZ.status == "partial"
    and .cells.PERF.status == "partial"
    and .cells.CONF.status == "partial"
    and .cells.DOC.status == "verified")
' testing/stdlib-owner-evidence.json >/dev/null

jq -e '
  any(.owners[]; .id == "std.testing"
    and .group == "a4-testing"
    and .operation == "std.testing.generate_diff"
    and .state == "captured-partial"
    and .workload == "representative")
' testing/stdlib-performance-conformance.json >/dev/null

echo "std.testing owner tests: OK"
