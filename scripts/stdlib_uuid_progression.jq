def uuid_conformance_progression($impl):
  if .conformance == null then
    .promotion.next_blocks == ["STD-UUID-CONF-001"]
    and $impl.required_follow_ups == ["STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  else
    (.conformance | del(.status, .quality_gate)) == {
      task:"STD-UUID-CONF-001", register:"testing/stdlib-uuid-conformance.json",
      contract:"docs/contracts/stdlib-uuid-conformance.md",
      selected_route:"public-hosted-vm-and-native-kernel-process",
      target:"x86_64-unknown-linux-gnu", cases:5, common_observations:77,
      public_api:"verified-production-hosted", vm_capability_checks:15,
      native_abi:"not-implemented", native_aot:"not-claimed"}
    and (if .conformance.status == "adapter-ready" then
      .conformance.quality_gate == "pending-80-percent-per-scope"
      and .promotion.next_blocks == ["STD-UUID-CONF-001"]
      and $impl.required_follow_ups == ["STD-UUID-CONF-001", "STD-UUID-DOC-001"]
    elif .conformance.status == "verified-public-hosted-vm-and-native-kernel-process" then
      .conformance.quality_gate == "verified-80-percent-per-scope"
      and .promotion.next_blocks == ["STD-UUID-DOC-001"]
      and $impl.required_follow_ups == ["STD-UUID-DOC-001"]
    else false end)
  end;

def uuid_kernel_progression:
  .implementation as $impl
  | $impl.public_api_promoted == false
  and $impl.native_aot_lowering == "not-claimed"
  and $impl.selected_route == "rust-scalar-kernel"
  and $impl.dependency == {crate:"sha1",version:"=0.10.6",features:["force-soft"],assembly:false}
  and $impl.generator_inputs == {
    v4:"exact-16-supplied-entropy-bytes",
    v5:"namespace-network-bytes-and-opaque-name",
    v7:"checked-i128-unix-milliseconds-and-exact-10-supplied-entropy-bytes"
  }
  and $impl.sources == ["crates/tondo-stdlib/src/uuid.rs", "crates/tondo-stdlib/src/lib.rs"]
  and ($impl.tests | length) == 18
  and ($impl.tests | unique | length) == 18
  and all($impl.tests[]; test("^crates/tondo-stdlib/src/uuid\\.rs::[a-z0-9_]+$"))
  and ($impl.proof | type == "string" and length > 0)
  and .documentation == null
  and (.conformance == null or (.measurement.status == "verified-hosted-scalar-baseline"
    and .model.status == "verified" and .host.status == "verified-production-hosted"))
  and (if .host != null then
    $impl.status == "verified-stdlib-kernel"
    and $impl.quality_gate == "verified-80-percent-per-scope"
    and .host.task == "STD-UUID-HOST-001"
    and .host.contract == "docs/contracts/stdlib-uuid-host.md"
    and .host.register == "testing/stdlib-uuid-host.json"
    and .host.selected_route == "hosted-scalar"
    and $impl.host == .host.status
    and (if .host.status == "ready-production-hosted" then
      .model == null and .measurement == null
      and .host.quality_gate == "pending-80-percent-per-scope"
      and $impl.runtime_heap == "ready-hosted-admission"
      and .promotion.next_blocks == ["STD-UUID-HOST-001"]
      and $impl.required_follow_ups == ["STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
    elif .host.status == "verified-production-hosted" then
      .host.quality_gate == "verified-80-percent-per-scope"
      and $impl.runtime_heap == "verified-hosted-admission"
      and (if .measurement != null then
        .model == {task:"STD-UUID-TEST-001", register:"testing/stdlib-uuid-test.json",
          contract:"docs/contracts/stdlib-uuid-test.md", status:"verified",
          quality_gate:"verified-80-percent-per-scope",
          selected_route:"independent-reference-and-kernel-hosted-regression-boundary"}
        and (.measurement | del(.status, .quality_gate)) == {
          task:"STD-UUID-PERF-001", register:"testing/stdlib-uuid-performance.json",
          contract:"docs/contracts/stdlib-uuid-performance.md", selected_route:"hosted-scalar",
          backend:"rust-hosted-bridge", target:"x86_64-unknown-linux-gnu", profile:"test",
          workloads:22, samples_per_workload:27, hosted_vm_timing:"not-measured",
          native_aot:"not-measured"}
        and (if .measurement.status == "measurement-ready" then
          .measurement.quality_gate == "pending-80-percent-per-scope"
          and .promotion.next_blocks == ["STD-UUID-PERF-001"]
          and $impl.required_follow_ups == ["STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
        elif .measurement.status == "verified-hosted-scalar-baseline" then
          .measurement.quality_gate == "verified-80-percent-per-scope"
          and uuid_conformance_progression($impl)
        else false end)
      elif .model != null then
        $impl.required_follow_ups == ["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
        and
        (.model | del(.status, .quality_gate)) == {
          task:"STD-UUID-TEST-001", register:"testing/stdlib-uuid-test.json",
          contract:"docs/contracts/stdlib-uuid-test.md",
          selected_route:"independent-reference-and-kernel-hosted-regression-boundary"}
        and (if .model.status == "ready" then
          .model.quality_gate == "pending-80-percent-per-scope"
          and .promotion.next_blocks == ["STD-UUID-TEST-001"]
        elif .model.status == "verified" then
          .model.quality_gate == "verified-80-percent-per-scope"
          and .promotion.next_blocks == ["STD-UUID-PERF-001"]
        else false end)
      else .promotion.next_blocks == ["STD-UUID-TEST-001"]
        and $impl.required_follow_ups == ["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"] end)
    else false end)
  else
    .model == null and .measurement == null
    and $impl.host == "not-claimed-until-uuid-host"
    and $impl.runtime_heap == "not-claimed"
    and (if $impl.status == "ready-stdlib-kernel" then
    $impl.quality_gate == "pending-80-percent-per-scope"
    and .promotion.next_blocks == ["STD-UUID-IMPL-001"]
    and $impl.required_follow_ups == ["STD-UUID-IMPL-001", "STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  elif $impl.status == "verified-stdlib-kernel" then
    $impl.quality_gate == "verified-80-percent-per-scope"
    and .promotion.next_blocks == ["STD-UUID-HOST-001"]
    and $impl.required_follow_ups == ["STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
    else false end)
  end);
