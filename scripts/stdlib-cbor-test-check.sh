#!/usr/bin/env bash
set -euo pipefail
root="$(cd "${BASH_SOURCE[0]%/*}/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_CBOR_TEST_CONTRACT:-testing/stdlib-cbor-test.json}"
corpus="${TONDO_STDLIB_CBOR_TEST_CORPUS:-crates/tondo-reliability/tests/fixtures/cbor-wire.json}"
die() { echo "std.cbor tests: $*" >&2; exit 1; }
[[ -f "$contract" && -f "$corpus" ]] || die "missing testing inputs"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "contract must end with LF"
jq -e '
  .format == "tondo-stdlib-cbor-testing/1" and .owner == "std.cbor"
  and .edition == "0.1" and .phase == "STD-0.1B" and .task == "STD-CBOR-TEST-001"
  and .status == "verified" and .contract == "docs/contracts/stdlib-cbor-test.md"
  and .parent_contract == "testing/stdlib-cbor.json"
  and .target == "independent-reference-and-rust-kernel-regression-boundary"
  and .limits == {max_reference_nodes:128, max_reference_depth:8,
    max_reference_scalar_bytes:96, max_fuzz_input_bytes:4096, max_fuzz_steps:512,
    model_seed_count:4096, float16_pattern_count:65536, fuzz_smoke_runs:128}
  and .model.status == "verified" and .model.production_imports == false
  and .model.source == "crates/tondo-reliability/src/cbor_model.rs"
  and .model.oracle == "independent bounded recursive wire grammar and arithmetic float renderer"
  and (.model.laws | length) == 10
  and .corpus == {path:"crates/tondo-reliability/tests/fixtures/cbor-wire.json",
    valid_cases:61, invalid_cases:30, source:"https://www.rfc-editor.org/rfc/rfc8949.html",
    scope:"bounded-wire-vectors-and-owner-regressions"}
  and .test.status == "verified" and .test.integration_tests == 9 and .test.kernel_tests == 14
  and .test.source == "crates/tondo-reliability/tests/cbor_models.rs"
  and .test.command == "cargo test -p tondo-reliability --test cbor_models --locked"
  and (.test.cases | length) == 11
  and .fuzz.status == "verified" and .fuzz.target == "stdlib_cbor"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_cbor.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_cbor/seed"
  and .fuzz.input_limit_bytes == 4096 and .fuzz.step_limit == 512
  and .fuzz.smoke == {runs:128, seed:4113, toolchain:"nightly-2026-07-28", result:"passed"}
  and .fuzz.timeout_seconds == 10 and .fuzz.rss_limit_mb == 4096
  and .fuzz.minimal_dependency_graph == "stdlib-only-no-compiler-vm-conformance-or-reliability-cli"
  and .promotion == {model_test_fuzz_complete:true, public_api_promoted:false,
    production_host_registration:"not-claimed", native_abi:"not-claimed", native_aot:"not-claimed",
    simd:"not-claimed", performance:"not-claimed", next_blocks:["STD-CBOR-PERF-001"],
    remaining:["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]}
' "$contract" >/dev/null || die "invalid testing contract"
jq -e '
  .format == "tondo-stdlib-cbor-corpus/1"
  and .source == "https://www.rfc-editor.org/rfc/rfc8949.html"
  and .scope == "bounded-wire-vectors-and-owner-regressions"
  and (.valid | length) == 61 and (.invalid | length) == 30
  and ([.valid[].id, .invalid[].id] | unique | length) == 91
  and all(.valid[]; (.wire | test("^([0-9a-f]{2})+$"))
    and (.ordinary | test("^([0-9a-f]{2})+$"))
    and ((has("deterministic") and (has("deterministic_error") | not)
       and (.deterministic | test("^([0-9a-f]{2})+$")))
      or ((has("deterministic") | not) and .deterministic_error == "KeyCollision")))
  and all(.invalid[]; (.wire | test("^([0-9a-f]{2})*$")) and (.error | type == "string" and length > 0))
' "$corpus" >/dev/null || die "invalid persistent wire corpus"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing linked input: $path"
done < <(jq -r '.contract, .parent_contract, .model.source, .corpus.path, .test.source, .fuzz.source, .fuzz.corpus' "$contract")
for path in scripts/stdlib-cbor-test-check.sh scripts/stdlib-cbor-test-test.sh scripts/stdlib-cbor-fuzz.sh; do
    [[ -x "$path" ]] || die "runner is not executable: $path"
done
[[ -s fuzz/corpus/stdlib_cbor/seed ]] || die "missing fuzz seed"
! rg -q 'tondo_stdlib|tondo_compiler|tondo_vm' crates/tondo-reliability/src/cbor_model.rs \
    || die "reference model imports production code"
rg -Fq 'name = "stdlib_cbor"' fuzz/Cargo.toml || die "missing fuzz target"
rg -Fq 'stdlib-cbor-test.md' docs/contracts/stdlib-cbor.md || die "missing parent documentation link"
rg -Fq 'stdlib-cbor-test.json' TONDO_STANDARD_LIBRARY_SPEC.md || die "missing normative register link"
echo "std.cbor tests: OK (independent wire oracle, persistent corpus, policies, limits and bounded fuzz)"
