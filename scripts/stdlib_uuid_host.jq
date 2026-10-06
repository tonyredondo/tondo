def uuid_host_boundary:
  .format == "tondo-stdlib-host-boundary/1"
  and .owner == "std.uuid" and .task == "STD-UUID-HOST-001"
  and .parent == "testing/stdlib-uuid.json"
  and .contract == "docs/contracts/stdlib-uuid-host.md"
  and .selected_route == "hosted-scalar" and .target == "tondo-vm-hosted"
  and .public_registration == "implemented-four-nominals-fourteen-operations"
  and .public_api_promoted == false
  and .storage == {
    carrier:"private-high-low-UInt64-record", network_order:"big-endian", width_bits:128,
    capabilities:["Copy", "Discard", "Equatable", "Key", "Send", "Share"], host_handle:false
  }
  and .providers.entropy == {
    dependency:{crate:"getrandom",version:"=0.4.3",features:[]},operation:"getrandom::fill",
    calls_per_generation:1,v4_bytes:16,v7_bytes:10,unsupported:"EntropyUnavailable",
    other_failure:"EntropyFailure",partial_bytes_published:false,tondo_retry:false
  }
  and .providers.clock == {
    operation:"std::time::SystemTime::now",
    conversion:"reject-pre-epoch-before-positive-millisecond-truncation",
    range:[0,281474976710655],precedes_entropy:true,timezone_lookup:false,
    strict_monotonicity:false,os_failure_result:"SystemTime::now-has-no-fallible-result"
  }
  and .capabilities == {
    core:[],v4:["entropy"],v7:["civil-clock", "entropy"],missing:"static-E1008",
    checked_references:["direct-call", "function-value", "defer"],import_effect:"none",
    suspends:false,selectable:false
  }
  and .testing_provider == {
    boundary:"sealed-Rust-EnvelopeHandle-input",public_tondo_setter:false,
    install_phase:"Setup-only-once",clock_rows:256,entropy_rows:256,entropy_row_bytes:16,
    consume:"once-shared-by-envelope-clones",exhaustion:"nominal-unavailable-no-OS-fallback",
    invalid_length:"ProviderMisconfigured-before-copy",
    close:"discard-fixture-and-refuse-further-access",
    budget:"atomic-work-and-logical-memory-admission-per-phase",capabilities_granted:false
  }
  and .admission == {
    detached_reply_max_bytes:182,typed_vm_result:"complete-success-and-error-storage-before-provider",
    limited_resources:["vm-heap-bytes", "vm-heap-objects", "target-provider-bytes", "test-work", "test-logical-memory"],
    to_bytes:"fresh-copy-reserve-before-publish",to_string:"fallible-36-byte-reserve-before-publish",
    provider_state_retained_by_uuid:false,clock_or_entropy_consumed_on_admission_refusal:false,
    memory_measurement:"logical-accounting-not-RSS-or-allocator-call-count"
  }
  and .errors == {
    carrier:"UuidError-record-kind-closed-enum-offset-Int-option",
    display:"kind-name-with-optional-at-byte-offset",platform_details:false,partial_success:false
  }
  and (.sources | length) == 16 and (.sources | unique | length) == 16
  and .fixtures == ["crates/tondo-compiler/tests/fixtures/uuid-core.to"]
  and (.tests | length) == 23 and (.tests | unique | length) == 23
  and all(.tests[]; test("^crates/tondo-(compiler|vm)/src/[a-z/_]+\\.rs::uuid_[a-z0-9_]+$"))
  and ([.tests[] | split("::")[0]] - .sources | length) == 0
  and .not_claimed == ["native-runtime-ABI", "native-AOT", "independent-model", "fuzz", "performance", "SIMD", "conformance-promotion", "usage-documentation"]
  and (.proof | type == "string" and length > 0)
  and (if .status == "ready-production-hosted" then
    .quality_gate == "pending-80-percent-per-scope"
    and .required_follow_ups == ["STD-UUID-HOST-001", "STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  elif .status == "verified-production-hosted" then
    .quality_gate == "verified-80-percent-per-scope"
    and .required_follow_ups == ["STD-UUID-TEST-001", "STD-UUID-PERF-001", "STD-UUID-CONF-001", "STD-UUID-DOC-001"]
  else false end);
