#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_REGEX_TEST_CONTRACT:-testing/stdlib-regex-test.json}"
corpus="${TONDO_STDLIB_REGEX_TEST_CORPUS:-crates/tondo-reliability/tests/fixtures/regex-cases.json}"
die() { echo "std.regex tests: $*" >&2; exit 1; }
[[ -f "$contract" && -f "$corpus" ]] || die "missing testing inputs"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
jq -e '
  .format == "tondo-stdlib-regex-testing/1" and .owner == "std.regex"
  and .edition == "0.1" and .phase == "STD-0.1B" and .task == "STD-REGEX-TEST-001"
  and .contract == "docs/contracts/stdlib-regex-test.md"
  and .parent_contract == "testing/stdlib-regex.json"
  and .target == "independent-reference-and-rust-kernel-regression-boundary"
  and ((.status == "ready" and .quality_gate == "pending-80-percent-per-scope"
        and .promotion.test_boundary_promoted == false)
    or (.status == "verified" and .quality_gate == "verified-80-percent-per-scope"
        and .promotion.test_boundary_promoted == true))
  and .limits == {max_reference_pattern_bytes:128, max_reference_nodes:128,
    max_reference_depth:8, max_reference_captures:8, max_reference_repeat:8,
    max_reference_input_bytes:96, max_reference_input_scalars:32,
    max_reference_path_steps:65536, max_reference_output_bytes:4096,
    max_reference_replacement_bytes:128,
    max_fuzz_input_bytes:4096, max_fuzz_steps:512, model_seed_count:4096, fuzz_smoke_runs:128}
  and .model.status == "verified" and .model.production_imports == false
  and .model.sources == ["crates/tondo-reliability/src/regex_model.rs", "crates/tondo-reliability/src/regex_model/grammar.rs"]
  and .model.oracle == "independent bounded recursive grammar and ordered path enumeration"
  and .model.unicode_oracle == "ascii-tables-and-unfolded-unicode-literals-not-full-property-tables"
  and .model.laws == [
    "leftmost start and local greedy/lazy alternative priorities",
    "full matching filters end of input before selection",
    "absent and participating empty captures remain distinct",
    "nullable repeats terminate and preserve prior captures",
    "half-open scalar-boundary UTF-8 spans",
    "zero-width enumeration advances one scalar including EOF once",
    "replacement templates validate before matching with atomic output",
    "reference limits and outside-domain results are not production rejections",
    "identical seeds produce identical cases and replay summaries"]
  and .corpus == {path:"crates/tondo-reliability/tests/fixtures/regex-cases.json",
    valid_cases:41, invalid_cases:33, model_cases:32, authored_unicode_cases:9,
    source:"docs/contracts/stdlib-regex.md", unicode:"16.0.0",
    scope:"bounded-model-and-separately-authored-unicode-vectors"}
  and .test.status == "verified" and .test.integration_tests == 10 and .test.kernel_tests == 10
  and .test.source == "crates/tondo-reliability/tests/regex_models.rs"
  and .test.command == "cargo test -p tondo-reliability --test regex_models --locked"
  and .fuzz.status == "verified" and .fuzz.target == "stdlib_regex"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_regex.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_regex/seed"
  and .fuzz.input_limit_bytes == 4096 and .fuzz.step_limit == 512
  and .fuzz.smoke == {runs:128, seed:4113, toolchain:"nightly-2026-07-28", result:"passed"}
  and .fuzz.timeout_seconds == 10 and .fuzz.rss_limit_mb == 4096
  and .fuzz.command == "TONDO_REGEX_FUZZ_RUNS=128 scripts/stdlib-regex-fuzz.sh"
  and .fuzz.minimal_dependency_graph == "stdlib-only-no-compiler-vm-conformance-or-reliability-cli"
  and (.promotion | del(.test_boundary_promoted)) == {public_api_promoted:false,
    production_vm_registration:"not-claimed", native_abi:"not-claimed", native_aot:"not-claimed",
    simd:"not-claimed", performance:"not-claimed", next_blocks:["STD-REGEX-PERF-001"],
    remaining:["STD-REGEX-PERF-001","STD-REGEX-CONF-001","STD-REGEX-DOC-001"]}
' "$contract" >/dev/null || die "invalid testing contract"
jq -e '
  def span: type == "array" and length == 2 and all(.[]; type == "number" and . >= 0 and floor == .) and .[0] <= .[1];
  .format == "tondo-stdlib-regex-corpus/1" and .source == "docs/contracts/stdlib-regex.md"
  and .unicode == "16.0.0" and .scope == "bounded-model-and-separately-authored-unicode-vectors"
  and (.valid | length) == 41 and (.invalid | length) == 33
  and ([.valid[].id, .invalid[].id] | unique | length) == 74
  and ([.valid[] | select(.model)] | length) == 32
  and all(.valid[]; (.id | type == "string" and length > 0)
    and (.pattern | type == "string") and (.input | type == "string")
    and (.options | type == "number" and . >= 0 and . < 32 and floor == .)
    and (.model | type == "boolean") and (.full | type == "boolean")
    and (.first == null or (.first | type == "array" and length > 0 and all(.[]; . == null or span)))
    and ((has("all") | not) or (.all | type == "array" and all(.[]; span)))
    and ((has("replacement") | not) or (.replacement | .template as $t | .first as $f | .all as $a
      | all([$t,$f,$a][]; type == "string"))))
  and all(.invalid[]; (.id | type == "string" and length > 0)
    and (.pattern | type == "string") and (.error | type == "string" and length > 0))
' "$corpus" >/dev/null || die "invalid persistent regex corpus"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done < <(jq -r '.contract, .parent_contract, .model.sources[], .corpus.path, .test.source, .fuzz.source, .fuzz.corpus' "$contract")
for path in scripts/stdlib-regex-test-check.sh scripts/stdlib-regex-test-test.sh scripts/stdlib-regex-fuzz.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
for path in crates/tondo-reliability/src/regex_model.rs crates/tondo-reliability/src/regex_model/grammar.rs; do
    if grep -Eq 'tondo_stdlib|tondo_compiler|tondo_vm|regex_syntax' "$path"; then
        die "reference model imports production code"
    else
        [[ "$?" == 1 ]] || die "reference model could not be inspected"
    fi
done
[[ "$(grep -c '^#\[test\]' crates/tondo-reliability/tests/regex_models.rs)" == 10 ]] || die "incomplete model tests"
[[ -s fuzz/corpus/stdlib_regex/seed ]] || die "missing fuzz seed"
grep -Fq 'name = "stdlib_regex"' fuzz/Cargo.toml || die "missing fuzz target"
grep -Fq 'stdlib-regex-test.md' docs/contracts/stdlib-regex.md || die "missing parent documentation link"
grep -Fq 'stdlib-regex-test.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing normative register link"
echo "std.regex tests: OK ($(jq -r .status "$contract"); bounded oracle, retained corpus, kernel-only fuzz)"
