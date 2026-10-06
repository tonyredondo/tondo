def uuid_kernel_progression:
  .implementation as $impl
  | $impl.public_api_promoted == false
  and $impl.host == "not-claimed-until-uuid-host"
  and $impl.native_aot_lowering == "not-claimed"
  and $impl.runtime_heap == "not-claimed"
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
  and .model == null and .measurement == null and .conformance == null and .documentation == null
  and (if $impl.status == "ready-stdlib-kernel" then
    $impl.quality_gate == "pending-80-percent-per-scope"
    and .promotion.next_blocks == ["STD-UUID-IMPL-001"]
    and $impl.required_follow_ups == ["STD-UUID-IMPL-001", "STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  elif $impl.status == "verified-stdlib-kernel" then
    $impl.quality_gate == "verified-80-percent-per-scope"
    and .promotion.next_blocks == ["STD-UUID-HOST-001"]
    and $impl.required_follow_ups == ["STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  else false end);
