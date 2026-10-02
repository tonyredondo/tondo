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

def cbor_owner_progression:
  (.performance | has("task") or has("contract") or has("status")) as $performance_claim
  | if .implementation.status == "kernel-verified-quality-pending" then
      .implementation.quality_gate == "pending-80-percent-per-scope"
      and .testing_contract == null and ($performance_claim | not)
      and .implementation.required_follow_ups == ["STD-CBOR-IMPL-001","STD-CBOR-TEST-001","STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
      and .promotion.next_blocks == ["STD-CBOR-IMPL-001"]
    elif .implementation.status == "verified-stdlib-kernel" then
      .implementation.quality_gate == "verified-80-percent-per-scope"
      and (if .testing_contract == null then
        ($performance_claim | not)
        and .implementation.required_follow_ups == ["STD-CBOR-TEST-001","STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
        and .promotion.next_blocks == ["STD-CBOR-TEST-001"]
      elif .testing_contract == "testing/stdlib-cbor-test.json" then
        if $performance_claim then
          cbor_performance_metadata
          and (if .performance.status == "verified-stdlib-kernel-baseline" then
            .implementation.required_follow_ups == ["STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
            and .promotion.next_blocks == ["STD-CBOR-CONF-001"]
          elif .performance.status == "measurement-ready" then
            .implementation.required_follow_ups == ["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
            and .promotion.next_blocks == ["STD-CBOR-PERF-001"]
          else false end)
        else
          .implementation.required_follow_ups == ["STD-CBOR-PERF-001","STD-CBOR-CONF-001","STD-CBOR-DOC-001"]
          and .promotion.next_blocks == ["STD-CBOR-PERF-001"]
        end
      else false end)
    else false end;
