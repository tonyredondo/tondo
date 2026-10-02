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
      and .implementation.quality_gate == "pending-80-percent-per-scope"
      and .promotion.next_blocks == ["STD-REGEX-IMPL-001"]
      and .implementation.required_follow_ups == ["STD-REGEX-IMPL-001", "STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"])
    or
    (.implementation.status == "verified-stdlib-kernel"
      and .implementation.quality_gate == "verified-80-percent-per-scope"
      and ((.testing_contract == null
        and .promotion.next_blocks == ["STD-REGEX-TEST-001"]
        and .implementation.required_follow_ups == ["STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"])
      or (.testing_contract == "testing/stdlib-regex-test.json"
        and .promotion.next_blocks == ["STD-REGEX-PERF-001"]
        and .implementation.required_follow_ups == ["STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"])))
  );
