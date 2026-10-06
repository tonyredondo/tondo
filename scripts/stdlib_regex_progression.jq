def regex_documentation_metadata:
  .documentation == {
    task: "STD-REGEX-DOC-001",
    status: .documentation.status,
    document: "docs/contracts/stdlib-regex.md",
    example: "crates/tondo-stdlib/examples/regex_usage.rs",
    command: "scripts/stdlib-regex-doc-check.sh",
    expected_stdout: "regex-doc-ok",
    examples: ["patterns-and-reuse", "unicode-and-options", "captures-and-spans", "lazy-iteration-and-ownership", "replacement-and-errors", "limits-and-costs"],
    sections: ["patterns-and-reuse", "unicode-and-options", "captures-and-utf8-spans", "lazy-iteration-and-ownership", "replacement-and-errors", "limits-and-costs", "executable-kernel-example", "promotion-boundary"],
    public_tondo_api: "not-implemented",
    production_vm_registration: "not-claimed",
    native_abi: "not-implemented",
    native_aot: "not-claimed"
  };

def regex_documentation_progression:
  if .documentation == null then
    .promotion.next_blocks == ["STD-REGEX-DOC-001"]
    and .implementation.required_follow_ups == ["STD-REGEX-DOC-001"]
  else
    regex_documentation_metadata
    and (if .documentation.status == "usage-ready" then
      .promotion.next_blocks == ["STD-REGEX-DOC-001"]
      and .implementation.required_follow_ups == ["STD-REGEX-DOC-001"]
    elif .documentation.status == "verified-rust-kernel-usage" then
      .promotion.next_blocks == ["STD-UUID-IMPL-001"]
      and .implementation.required_follow_ups == []
    else false end)
  end;

def regex_conformance_metadata:
  (.conformance | keys) == ["contract", "document", "hosted_vm", "native_abi", "native_aot", "native_process", "status", "task"]
  and .conformance.contract == "testing/stdlib-regex-conformance.json"
  and .conformance.document == "docs/contracts/stdlib-regex-conformance.md"
  and .conformance.task == "STD-REGEX-CONF-001"
  and .conformance.hosted_vm == "verified-bytecode-test-only-host-callable"
  and .conformance.native_process == "rust-stdlib-process-no-regex-abi"
  and .conformance.native_abi == "not-implemented"
  and .conformance.native_aot == "not-claimed";

def regex_performance_follow_ups:
  if .conformance == null then
    .documentation == null and .promotion.next_blocks == ["STD-REGEX-CONF-001"]
    and .implementation.required_follow_ups == ["STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]
  else
    regex_conformance_metadata
    and (if .conformance.status == "adapter-ready" then
      .documentation == null and .promotion.next_blocks == ["STD-REGEX-CONF-001"]
      and .implementation.required_follow_ups == ["STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]
    elif .conformance.status == "verified-hosted-vm-adapter-and-native-stdlib-process" then
      regex_documentation_progression
    else false end)
  end;

def regex_performance_metadata:
  (.performance | keys) == ["allocation", "backend", "claims_before_perf_gate", "contract", "dispatch", "document", "hosted_vm", "matching", "native_aot", "parser_stack", "profile", "samples_per_workload", "scalar_oracle", "selected_dispatch", "simd_allowed_after_equivalence", "status", "target", "task", "workloads"]
  and .performance.task == "STD-REGEX-PERF-001"
  and .performance.contract == "testing/stdlib-regex-performance.json"
  and .performance.document == "docs/contracts/stdlib-regex-performance.md"
  and .performance.target == "x86_64-unknown-linux-gnu"
  and .performance.backend == "rust-stdlib-kernel"
  and .performance.profile == "test"
  and .performance.workloads == 19
  and .performance.samples_per_workload == 27
  and .performance.selected_dispatch == "scalar-fixed-target"
  and .performance.native_aot == "not-claimed"
  and .performance.hosted_vm == "not-claimed-no-regex-bridge";

def regex_without_performance_child:
  (.performance | keys) == ["allocation", "claims_before_perf_gate", "dispatch", "matching", "parser_stack", "scalar_oracle", "simd_allowed_after_equivalence"];

def regex_tested_progression:
  .testing_contract == "testing/stdlib-regex-test.json"
  and (if regex_without_performance_child then
    .conformance == null and .documentation == null and .promotion.next_blocks == ["STD-REGEX-PERF-001"]
    and .implementation.required_follow_ups == ["STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]
  else
    regex_performance_metadata
    and (if .performance.status == "measurement-ready" then
      .conformance == null and .documentation == null and .promotion.next_blocks == ["STD-REGEX-PERF-001"]
      and .implementation.required_follow_ups == ["STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]
    elif .performance.status == "verified-stdlib-kernel-baseline" then
      regex_performance_follow_ups
    else false end)
  end);

def regex_kernel_progression:
  .implementation.public_api_promoted == false
  and .implementation.host == "not-applicable-pure-core"
  and .implementation.production_vm_registration == "not-claimed"
  and .implementation.native_abi == "not-claimed"
  and .implementation.native_aot_lowering == "not-claimed"
  and .implementation.selected_route == "rust-kernel-ordered-thompson-nfa"
  and .implementation.parser_dependency == {name: "regex-syntax", version: "=0.8.10", unicode: "16.0.0"}
  and .implementation.heap_metric == "conservative-logical-storage-not-rss-or-vm-allocator"
  and .implementation.system_oom_recovery == "not-claimed-for-dependency-or-rust-global-allocator"
  and .implementation.sources == ["crates/tondo-stdlib/src/regex.rs", "crates/tondo-stdlib/src/regex/syntax.rs", "crates/tondo-stdlib/src/regex/engine.rs", "crates/tondo-stdlib/src/lib.rs"]
  and .implementation.tests == [
    "crates/tondo-stdlib/tests/regex_kernel.rs::local_priorities_and_full_search_are_distinct",
    "crates/tondo-stdlib/tests/regex_kernel.rs::captures_preserve_optional_and_empty_repeated_groups",
    "crates/tondo-stdlib/tests/regex_kernel.rs::unicode_16_properties_simple_folding_and_exact_word_set",
    "crates/tondo-stdlib/tests/regex_kernel.rs::line_anchors_dot_and_options_are_explicit",
    "crates/tondo-stdlib/tests/regex_kernel.rs::iterator_is_lazy_fallible_cumulative_and_fused",
    "crates/tondo-stdlib/tests/regex_kernel.rs::replacement_templates_and_zero_width_progress_are_atomic",
    "crates/tondo-stdlib/tests/regex_kernel.rs::closed_dialect_rejects_extensions_and_reports_spans",
    "crates/tondo-stdlib/tests/regex_kernel.rs::compile_and_match_limits_are_nominal_and_fingerprints_are_stable",
    "crates/tondo-stdlib/tests/regex_kernel.rs::bounded_ambiguous_and_nested_patterns_terminate_without_recursion",
    "crates/tondo-stdlib/tests/regex_kernel.rs::malformed_unicode_patterns_never_panic_or_split_error_boundaries"
  ]
  and .implementation.evidence_report == "target/reliability/evidence/stdlib-regex-implementation.json"
  and (
    (.implementation.status == "ready-stdlib-kernel"
      and .testing_contract == null and .conformance == null and .documentation == null and regex_without_performance_child
      and .implementation.quality_gate == "pending-80-percent-per-scope"
      and .promotion.next_blocks == ["STD-REGEX-IMPL-001"]
      and .implementation.required_follow_ups == ["STD-REGEX-IMPL-001", "STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"])
    or
    (.implementation.status == "verified-stdlib-kernel"
      and .implementation.quality_gate == "verified-80-percent-per-scope"
      and ((.testing_contract == null and .conformance == null and .documentation == null and regex_without_performance_child
        and .promotion.next_blocks == ["STD-REGEX-TEST-001"]
        and .implementation.required_follow_ups == ["STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"])
      or regex_tested_progression))
  );
