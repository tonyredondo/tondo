def log_testing_boundary:
  .format == "tondo-stdlib-log-testing/1" and .owner == "std.log"
  and .task == "STD-LOG-TEST-001"
  and .parent == "testing/stdlib-log.json"
  and .parent_contract == "docs/contracts/stdlib-log.md"
  and .contract == "docs/contracts/stdlib-log-test.md"
  and .target == "bounded-reference-kernel-and-public-hosted-sinks"
  and ((.status == "ready" and .quality_gate == "pending-current-source-proof"
        and .promotion.test_boundary_promoted == false)
    or (.status == "verified" and .quality_gate == "verified-80-percent-per-scope"
        and .promotion.test_boundary_promoted == true))
  and .model == {
    sources:["crates/tondo-reliability/src/log_model.rs",
      "crates/tondo-reliability/src/log_model/values.rs",
      "crates/tondo-reliability/src/log_model/queue.rs"],
    independent:true,production_imports:false,
    algorithm:"recursive-values-and-scripted-finite-queue",
    outside_domain:"OutsideDomain-not-Tondo-ResourceLimit",
    timestamp_validation:"caller-provided-canonical-tokens-not-calendar-or-clock",
    float_rendering:"shared-Rust-primitive-not-independent-numeric-formatting",
    unit_tests:14}
  and .limits == {
    max_reference_nodes:128,max_reference_scalar_bytes:256,max_reference_height:20,
    max_reference_output_bytes:262144,max_reference_queue:8,max_reference_record_bytes:2048,
    max_fuzz_input_bytes:4096,max_fuzz_steps:512,model_seed_count:4096}
  and .test == {
    sources:["crates/tondo-reliability/tests/log_kernel_models.rs",
      "crates/tondo-reliability/tests/log_corpus.rs",
      "crates/tondo-reliability/tests/log_hosted_models.rs"],
    integration_tests:9,corpus:"crates/tondo-reliability/tests/fixtures/log-cases.json",
    corpus_cases:34,kernel_comparison_adapter:"crates/tondo-reliability/src/log_fuzz.rs",
    record_formats:["Text","JsonLines"],limit_profiles:6,console_profiles:12,
    writer_profiles:7,concurrent_producers:4,records_per_producer:8,
    providers:"owned-files-and-controlled-public-or-sealed-hosted-writers",
    cancellation:"actual-Group.cancel-and-retained-prefix-resume",
    process_scope:"writable-cgroup-v2-for-process-bearing-workspace-tests",
    reference_queue_terminal_owners:0,reference_queue_terminal_bytes:0,writer_close_count:1}
  and .fuzz == {
    target:"stdlib_log",source:"fuzz/fuzz_targets/stdlib_log.rs",
    corpus:"fuzz/corpus/stdlib_log/seed",
    minimal_dependency_graph:"stdlib-only-no-compiler-vm-conformance-reliability",
    shared_model_and_kernel_adapter:true,sanitizer:"address",timeout_seconds:10,rss_limit_mb:4096,
    smoke:{result:"passed",runs:128,seed:4113,toolchain:"nightly-2026-07-28"}}
  and (.promotion | del(.test_boundary_promoted)) == {
    public_api_promoted:false,native_abi:"not-implemented",native_aot:"not-claimed",
    performance:"not-measured",full_owner:false,next_blocks:["STD-LOG-PERF-001"]};

def log_owner_progression:
  .model as $model
  | .measurement as $measurement
  | (.implementation.host != "verified-production-hosted"
    or .implementation.status == "verified-public-hosted-core")
  and (if $model == null then true else
    ($model | del(.status,.quality_gate)) == {
      task:"STD-LOG-TEST-001",register:"testing/stdlib-log-test.json",
      contract:"docs/contracts/stdlib-log-test.md",
      selected_route:"bounded-reference-kernel-and-public-hosted-sinks"}
    and (($model.status == "ready" and $model.quality_gate == "pending-current-source-proof")
      or ($model.status == "verified" and $model.quality_gate == "verified-80-percent-per-scope"
        and .implementation.status == "verified-public-hosted-core"
        and .implementation.host == "verified-production-hosted"))
  end)
  and (if $measurement == null then true else
    $model.status == "verified"
    and ($measurement | del(.status,.quality_gate)) == {
      register:"testing/stdlib-log-performance.json",contract:"docs/contracts/stdlib-log-performance.md",
      target:"x86_64-unknown-linux-gnu",backend:"hosted-bytecode-vm",profile:"test",
      samples_per_workload:27,native_abi:"unmeasured",native_aot:"unmeasured"}
    and (($measurement.status == "measurement-ready"
      and $measurement.quality_gate == "pending-80-percent-per-scope")
      or ($measurement.status == "verified-hosted-scalar-baseline"
        and $measurement.quality_gate == "verified-80-percent-per-scope"))
  end)
  and .promotion.next_blocks == (if .implementation.status == "core-implementation-in-progress"
    then ["STD-LOG-IMPL-001"]
    elif .implementation.host == "pending-STD-LOG-HOST-001" then ["STD-LOG-HOST-001"]
    elif $measurement.status == "verified-hosted-scalar-baseline" then ["STD-LOG-CONF-001"]
    elif $model.status == "verified" then ["STD-LOG-PERF-001"]
    else ["STD-LOG-TEST-001"] end);
