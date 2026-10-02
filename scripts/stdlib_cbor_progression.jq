# One owner progression rule shared by the parent and implementation checks.
def cbor_performance_metadata:
  .performance.task == "STD-CBOR-PERF-001"
  and .performance.contract == "testing/stdlib-cbor-performance.json"
  and .performance.document == "docs/contracts/stdlib-cbor-performance.md"
  and .performance.target == "x86_64-unknown-linux-gnu"
  and .performance.backend == "rust-stdlib-kernel"
  and .performance.profile == "test"
  and .performance.workloads == 15
  and .performance.samples_per_workload == 27
  and .performance.selected_dispatch == "scalar-fixed-target"
  and .performance.native_aot == "not-claimed"
  and .performance.hosted_vm == "not-claimed-no-cbor-bridge";

def cbor_documentation_metadata:
  .documentation == {
    task: "STD-CBOR-DOC-001",
    status: .documentation.status,
    document: "docs/contracts/stdlib-cbor.md",
    example: "crates/tondo-stdlib/examples/cbor_usage.rs",
    command: "scripts/stdlib-cbor-doc-check.sh",
    expected_stdout: "cbor-doc-ok",
    examples: ["materialized-and-typed", "tags-and-determinism", "raw-view-and-costs", "buffered-events-and-lifecycle", "errors-and-limits", "partial-io-is-not-transactional"],
    sections: ["values-and-byte-preservation", "tags-and-deterministic-encoding", "policies-and-limits", "errors-ownership-and-terminal-streams", "costs-and-performance", "executable-kernel-example", "promotion-boundary"],
    public_tondo_api: "not-implemented",
    native_aot: "not-claimed"
  };

def cbor_documentation_progression:
  if .documentation == null then
    .implementation.required_follow_ups == ["STD-CBOR-DOC-001"]
    and .promotion.next_blocks == ["STD-CBOR-DOC-001"]
  else
    cbor_documentation_metadata
    and (if .documentation.status == "usage-ready" then
      .implementation.required_follow_ups == ["STD-CBOR-DOC-001"]
      and .promotion.next_blocks == ["STD-CBOR-DOC-001"]
    elif .documentation.status == "verified-rust-kernel-usage" then
      .implementation.required_follow_ups == []
      and .promotion.next_blocks == ["STD-REGEX-IMPL-001"]
    else false end)
  end;

def cbor_conformance_progression:
  if .conformance == null then
    .documentation == null
    and .implementation.required_follow_ups == ["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
    and .promotion.next_blocks == ["STD-CBOR-CONF-001"]
  else
    .conformance.task == "STD-CBOR-CONF-001"
    and .conformance.contract == "testing/stdlib-cbor-conformance.json"
    and .conformance.document == "docs/contracts/stdlib-cbor-conformance.md"
    and .conformance.vm == "verified-bytecode-test-only-host-callable"
    and .conformance.native == "rust-stdlib-process-no-cbor-abi"
    and .conformance.public_api == "not-implemented"
    and .conformance.native_aot == "not-claimed"
    and (if .conformance.status == "adapter-ready" then
      .documentation == null
      and .implementation.required_follow_ups == ["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
      and .promotion.next_blocks == ["STD-CBOR-CONF-001"]
    elif .conformance.status == "verified-hosted-vm-adapter-and-native-stdlib-process" then
      cbor_documentation_progression
    else false end)
  end;

def cbor_owner_progression:
  (.performance | has("task") or has("contract") or has("status")) as $performance_claim
  | if .implementation.status == "kernel-verified-quality-pending" then
      .implementation.quality_gate == "pending-80-percent-per-scope"
      and .testing_contract == null and ($performance_claim | not)
      and .conformance == null and .documentation == null
      and .implementation.required_follow_ups == ["STD-CBOR-IMPL-001","STD-CBOR-TEST-001","STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
      and .promotion.next_blocks == ["STD-CBOR-IMPL-001"]
    elif .implementation.status == "verified-stdlib-kernel" then
      .implementation.quality_gate == "verified-80-percent-per-scope"
      and (if .testing_contract == null then
        ($performance_claim | not) and .conformance == null and .documentation == null
        and .implementation.required_follow_ups == ["STD-CBOR-TEST-001","STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
        and .promotion.next_blocks == ["STD-CBOR-TEST-001"]
      elif .testing_contract == "testing/stdlib-cbor-test.json" then
        if $performance_claim then
          cbor_performance_metadata
          and (if .performance.status == "verified-stdlib-kernel-baseline" then
            cbor_conformance_progression
          elif .performance.status == "measurement-ready" then
            .conformance == null and .documentation == null
            and .implementation.required_follow_ups == ["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
            and .promotion.next_blocks == ["STD-CBOR-PERF-001"]
          else false end)
        else
          .conformance == null and .documentation == null
          and .implementation.required_follow_ups == ["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
          and .promotion.next_blocks == ["STD-CBOR-PERF-001"]
        end
      else false end)
    else false end;
