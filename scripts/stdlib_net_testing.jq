def net_testing_boundary:
  .format == "tondo-stdlib-net-testing/1" and .owner == "std.net"
  and .edition == "0.1" and .phase == "STD-0.1B"
  and .task == "STD-NET-TEST-001"
  and .contract == "docs/contracts/stdlib-net-test.md"
  and .parent_contract == "testing/stdlib-net.json"
  and .target == "independent-reference-and-public-hosted-regression-boundary"
  and ((.status == "ready" and .quality_gate == "pending-current-source-proof"
        and .promotion.test_boundary_promoted == false)
    or (.status == "verified" and .quality_gate == "verified-80-percent-per-scope"
        and .promotion.test_boundary_promoted == true))
  and .limits == {max_reference_bytes:128, max_reference_addresses:32,
    max_fuzz_input_bytes:4096, max_fuzz_steps:512,
    model_seed_count:4096, fuzz_smoke_runs:128}
  and .model.sources == ["crates/tondo-reliability/src/net_model.rs",
    "crates/tondo-reliability/src/net_model/admission.rs",
    "crates/tondo-reliability/src/net_model/stream.rs",
    "crates/tondo-reliability/src/net_model/tls.rs"]
  and .model.production_imports == false
  and .model.outside_domain == "not-a-production-rejection"
  and .model.tls_oracle == "explicit-verdict-admission-and-affine-owner-ledger"
  and .model.os_dns_tls_packet_parser == false
  and .corpus == {path:"crates/tondo-reliability/tests/fixtures/net-cases.json",cases:40,
    scope:"bounded-admission-and-owner-regressions"}
  and .test.sources == ["crates/tondo-reliability/tests/net_kernel_models.rs",
    "crates/tondo-reliability/tests/net_corpus.rs",
    "crates/tondo-reliability/tests/net_hosted_models.rs"]
  and .test.independent_unit_tests == 14 and .test.integration_tests == 9
  and .test.providers == "controlled-loopback-no-external-service"
  and .test.tls_versions == ["1.2", "1.3"]
  and .test.process_scope == "explicit-delegated-scope"
  and .test.terminal_proof == "model-ledger-peer-eof-and-production-host-retirement"
  and .fuzz.target == "stdlib_net"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_net.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_net/seed"
  and .fuzz.input_limit_bytes == 4096 and .fuzz.step_limit == 512
  and .fuzz.smoke == {runs:128,seed:4113,toolchain:"nightly-2026-07-28",result:"passed"}
  and .fuzz.timeout_seconds == 10 and .fuzz.rss_limit_mb == 4096
  and .fuzz.minimal_dependency_graph == "stdlib-only-no-compiler-vm-conformance-or-reliability-cli"
  and .promotion.public_api_promoted == false
  and .promotion.native_abi == "not-claimed" and .promotion.native_aot == "not-claimed"
  and .promotion.performance == "not-claimed"
  and .promotion.next_blocks == ["STD-NET-PERF-001"];

def net_owner_progression:
  .host as $host | .model as $model
  | ($host.status == "implementation-in-progress" or $host.status == "verified-production-hosted")
  and $host.quality_gate == (if $host.status == "implementation-in-progress"
    then "pending-current-source-proof" else "verified-80-percent-per-scope" end)
  and (if $model == null then true else
    ($model | del(.status, .quality_gate)) == {
      task:"STD-NET-TEST-001",register:"testing/stdlib-net-test.json",
      contract:"docs/contracts/stdlib-net-test.md",
      selected_route:"independent-reference-and-public-hosted-regression-boundary"}
    and (($model.status == "ready" and $model.quality_gate == "pending-current-source-proof")
      or ($model.status == "verified" and $model.quality_gate == "verified-80-percent-per-scope"
        and $host.status == "verified-production-hosted"))
  end)
  and .promotion.next_blocks == (if .implementation.status == "ready-kernel-private-provider"
    then ["STD-NET-IMPL-001"]
    elif $host.status == "implementation-in-progress" then ["STD-NET-HOST-001"]
    elif $model.status == "verified" then ["STD-NET-PERF-001"]
    else ["STD-NET-TEST-001"] end);
