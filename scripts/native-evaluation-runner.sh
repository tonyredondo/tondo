#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="$root/testing/native-evaluation-runner.json"

die() {
    echo "native evaluation runner: $*" >&2
    exit 1
}

scripts/native-evaluation-runner-check.sh
scripts/native-evaluation-check.sh
scripts/native-evaluation-fast-check.sh
scripts/native-select-check.sh
scripts/native-select-test.sh
scripts/native-thread-check.sh
scripts/native-thread-test.sh
scripts/native-std-core-check.sh
scripts/native-std-core-test.sh
scripts/native-aot-lowering-check.sh
scripts/native-aot-lowering-test.sh
scripts/native-aot-binary-check.sh
scripts/native-aot-binary-test.sh
scripts/native-aot-memory-check.sh
scripts/native-aot-memory-test.sh

llvm_tool="${TONDO_LLVM_LLC:-/usr/bin/llc}"
cc_tool="${TONDO_NATIVE_CC:-/usr/bin/cc}"
strip_tool="${TONDO_NATIVE_STRIP:-/usr/bin/strip}"
readelf_tool="${TONDO_NATIVE_READELF:-/usr/bin/readelf}"
[[ "$llvm_tool" = /* && -x "$llvm_tool" ]] \
    || die "TONDO_LLVM_LLC must be an absolute executable"
[[ "$cc_tool" = /* && -x "$cc_tool" ]] \
    || die "TONDO_NATIVE_CC must be an absolute executable"
[[ "$strip_tool" = /* && -x "$strip_tool" ]] \
    || die "TONDO_NATIVE_STRIP must be an absolute executable"
[[ "$readelf_tool" = /* && -x "$readelf_tool" ]] \
    || die "TONDO_NATIVE_READELF must be an absolute executable"
llvm_version="$($llvm_tool --version 2>&1 | sed -n '1p')"
grep -Eq 'LLVM version 18\.' <<< "$llvm_version" \
    || die "LLVM llc must be version 18.x"

target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" != /* ]]; then
    target_dir="$root/$target_dir"
fi
evidence_dir="$target_dir/reliability/evidence"
adapter_target="$target_dir/native-evaluation"
tmp_root="$root/.tmp"
mkdir -p "$evidence_dir" "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-evaluation-runner.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

mapfile -t fixtures < <(jq -r '.corpus[].path' testing/native-evaluation-fast.json)
[[ "${#fixtures[@]}" -eq 4 ]] || die "expected four hash-pinned fixtures"
probe="$tmp/mir-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- "${fixtures[@]}" > "$probe"
std_core_probe="$tmp/std-core-mir-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-std-core-001.to > "$std_core_probe"

CARGO_TARGET_DIR="$adapter_target" cargo build \
    --manifest-path tools/native-evaluation/Cargo.toml --locked --quiet
adapter="$adapter_target/debug/tondo-native-evaluation"
[[ -x "$adapter" ]] || die "missing native evaluation adapter binary"

report="$evidence_dir/native-evaluation-runner.json"
performance_args=()
if [[ -n "${TONDO_NATIVE_AOT_PERF_OUTPUT:-}" ]]; then
    performance_args+=(--aot-performance-output "$TONDO_NATIVE_AOT_PERF_OUTPUT")
fi
"$adapter" \
    --probe "$probe" \
    --output "$report" \
    --llvm "$llvm_tool" \
    --target "$(rustc -vV | sed -n 's/^host: //p')" \
    --temp-dir "$tmp/backend" \
    --std-core-probe "$std_core_probe" \
    --cc "$cc_tool" \
    --strip "$strip_tool" \
    --readelf "$readelf_tool" \
    "${performance_args[@]}" \
    || die "native scalar runner failed"

jq -e '
  .format == "tondo-native-evaluation-candidates/1"
  and .phase == "NATIVE-001"
  and .status == "passed"
  and .adapter.format == "tondo-mir-backend/1"
  and ([.debug_metadata[] | select(.format == "tondo-mir-debug/1" and .sources >= 1 and .symbols >= 1 and .source_maps >= 1)] | length == 4)
  and .correctness.native_semantics == "scalar-and-managed-result-checked-arithmetic-logical-conversions-control-flow-host-calls-cleanup-ownership-async-thread-select-std-core-and-traps"
  and ([.native_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length >= 1)
  and ([.native_runs[] | select(.oracle_status == "trapped")] | length >= 1)
  and all(.native_runs[];
      ((.oracle_status == "returned"
        and .vm_status == "returned"
        and (.oracle_result | type == "number")
        and (.vm_result | type == "number")
        and (.oracle_result == .vm_result))
       or
       (.oracle_status == "trapped"
        and (.vm_status == "panicked" or .vm_status == "error")
        and .oracle_result == null
        and .vm_result == null))
      and ((.oracle_status == "returned" and .vm_diagnostics == [])
           or (.oracle_status == "trapped"
               and (.vm_diagnostics | length >= 1)
               and all(.vm_diagnostics[]; startswith("vm-"))))
  )
  and ([.native_managed_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length >= 1)
  and all(.native_managed_runs[];
      .oracle_status == "returned"
      and .vm_status == "returned"
      and (.oracle_tag | type == "number")
      and (.vm_tag | type == "number")
      and (.oracle_tag == .vm_tag)
      and (.oracle_payload == null or (.oracle_payload | type == "number"))
      and (.vm_payload == null or (.vm_payload | type == "number"))
  )
  and ([.native_runtime_runs[] | select(.case == "cleanup-exactly-once" and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "cleanup-abort" and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "ownership-cow" and .cranelift == "passed" and .llvm == "passed" and .expected_tag == 2 and .expected_payload == 42)] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "async-await" and .expected_result == 77 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "async-structured-join" and .expected_result == 0 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "async-scope-cancel" and .expected_result == 2 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "async-task-progress" and .expected_result == 1 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_runtime_runs[] | select(.case == "async-cancel-wake-rejected" and .expected_result == 3 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-ready-join" and .expected_result == 11 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-pending-wakeup" and .expected_result == 22 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-round-robin" and .expected_result == 1 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-rollback-ownership" and .expected_result == 2 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-oneshot" and .expected_result == 61 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-time" and .expected_result == 63 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-thread-join" and .expected_result == 74 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_select_runs[] | select(.case == "select-else" and .expected_result == 8 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_thread_runs[] | select(.case == "thread-worker-status" and .expected_result == 2 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_thread_runs[] | select(.case == "thread-worker-runs" and .expected_result == 1 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_thread_runs[] | select(.case == "thread-worker-distinct" and .expected_result == 1 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_thread_runs[] | select(.case == "thread-worker-join" and .expected_result == 94 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_thread_runs[] | select(.case == "thread-worker-cancel" and .expected_result == 2 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and ([.native_std_core_runs[] | select(.cranelift == "passed" and .llvm == "passed")] | length == 14)
  and ([.native_std_core_runs[].case] == [
    "option-some", "option-none", "option-unwrap-some", "option-unwrap-none",
    "option-map-some", "option-map-none", "result-ok", "result-err",
    "result-unwrap-ok", "result-unwrap-err", "result-map-ok", "result-map-err",
    "result-map-err-ok", "result-map-err-error"
  ])
  and all(.native_std_core_runs[];
    .kind | IN("scalar", "managed")
  )
  and all(.native_std_core_runs[];
    if .kind == "scalar" then
      (.oracle_result != null and .oracle_result == .vm_result)
    else
      (.oracle_result == null
       and .oracle_tag == .vm_tag
       and ((.oracle_payload == null and .vm_payload == null)
            or (.oracle_payload == (.vm_payload | tonumber))))
    end
  )
  and ([.native_lowering_runs[] | select(.case == "deferred-task-call" and .function_ordinal == 1 and .pending_before_join == 0 and .result_after_join == 42 and .joined_after_join == 3 and .cranelift == "passed" and .llvm == "passed")] | length == 1)
  and .native_diagnostics.format == "tondo-native-diagnostics/1"
  and .native_diagnostics.phase == "DIAG-NATIVE-001"
  and .native_diagnostics.status == "passed"
  and .native_diagnostics.oracle == "hosted-diagnostic-contract-fixtures"
  and .native_diagnostics.backends == ["cranelift", "llvm"]
  and ([.native_diagnostics.cases[].case] == [
    "race-conflict", "race-clean", "leak-growth", "leak-clean",
    "arc-cycle-reclaimed", "crash-dump", "crash-corruption-rejected",
    "crash-limit-enforced"
  ])
  and all(.native_diagnostics.cases[];
    .cranelift == "passed"
    and .llvm == "passed"
    and .envelope.format == "tondo-diagnostic-report/1"
    and (.envelope.status | IN("clean", "finding", "captured", "unsupported"))
    and .envelope.redacted == true
    and .envelope.payloads_omitted == true
  )
' "$report" >/dev/null || die "runner report did not prove native execution"

jq -e '
  .native_aot_lowering.format == "tondo-native-aot-lowering/1"
  and .native_aot_lowering.phase == "NATIVE-AOT-LOWER-001"
  and .native_aot_lowering.status == "passed"
  and .native_aot_lowering.mir_format == "tondo-mir-backend/1"
  and .native_aot_lowering.oracle == "normalized-MIR-reference-interpreter"
  and .native_aot_lowering.candidates == ["cranelift", "llvm"]
  and .native_aot_lowering.same_mir == true
  and ([.native_aot_lowering.feature_families[] | select(.cranelift == "passed" and .llvm == "passed" and .vm == "passed" and .cases >= 1)] | length == 10)
  and ([.native_aot_lowering.cases[] | select(.cranelift == "passed" and .llvm == "passed" and .vm_status == "returned" and .same_mir == true)] | length >= 27)
  and ([.native_aot_lowering.cases[].id] | index("array-storage")) != null
  and ([.native_aot_lowering.cases[].id] | index("closure-mutable-capture")) != null
  and ([.native_aot_lowering.cases[].id] | index("ownership-cow")) != null
  and ([.native_aot_lowering.traps[] | select(.candidate == "cranelift" and (.reason | contains("explicit-trap")))] | length == 1)
  and ([.native_aot_lowering.traps[] | select(.candidate == "llvm" and (.reason | contains("explicit-trap")))] | length == 1)
' "$report" >/dev/null || die "runner report did not prove complete AOT lowering inventory"

jq -e '
  .native_aot_binary.format == "tondo-native-aot-binary/1"
  and .native_aot_binary.phase == "NATIVE-AOT-BINARY-001"
  and .native_aot_binary.status == "passed"
  and .native_aot_binary.profile == "release"
  and .native_aot_binary.same_target_runtime_stdlib_linker_profile == true
  and (.native_aot_binary.shared_inputs.runtime_abi == "tondo-runtime-draft/1")
  and (.native_aot_binary.shared_inputs.stdlib == "STD-0.1A")
  and (.native_aot_binary.shared_inputs.linker_flags | index("-Wl,--build-id=none")) != null
  and all([.native_aot_binary.shared_inputs.mir_sha256,
           .native_aot_binary.shared_inputs.runtime_sha256,
           .native_aot_binary.shared_inputs.stdlib_sha256,
           .native_aot_binary.shared_inputs.target_descriptor_sha256,
           .native_aot_binary.shared_inputs.linker_sha256,
           .native_aot_binary.shared_inputs.strip_sha256,
           .native_aot_binary.shared_inputs.readelf_sha256][];
      test("^sha256:[0-9a-f]{64}$"))
  and ([.native_aot_binary.candidates[] | select(.status == "passed" and .reproducible == true and (.builds | length == 2))] | length == 2)
  and all(.native_aot_binary.candidates[];
      (.toolchain_sha256 | test("^sha256:[0-9a-f]{64}$"))
      and (.startup.product == "stripped")
      and (.startup.process_count == 3)
      and (.startup.samples_ns | length == 3)
      and all(.startup.samples_ns[]; . > 0)
      and (.startup.median_ns > 0)
      and (.startup.p95_ns >= .startup.median_ns)
      and (.startup.p99_ns >= .startup.p95_ns)
      and all(.builds[];
          .debug_bytes > 0
          and .stripped_bytes > 0
          and .debug_bytes >= .stripped_bytes
          and (.object_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and (.debug_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and (.stripped_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and (.receipt_sha256 | test("^sha256:[0-9a-f]{64}$"))
          and ([.debug_sections[], .stripped_sections[]] | map(select(.name == ".text" and .bytes > 0)) | length >= 2)
      )
  )
' "$report" >/dev/null || die "runner report did not prove comparable linked AOT products"

jq -e '
  .native_aot_memory.format == "tondo-native-aot-memory/1"
  and .native_aot_memory.phase == "NATIVE-AOT-MEM-001"
  and .native_aot_memory.status == "passed"
  and .native_aot_memory.oracle == "vm-semantics-native-instrumented-counters"
  and .native_aot_memory.protocol == {
      warmup_iterations: 3,
      measurement_repetitions: 9,
      independent_processes: 3,
      minimum_sample_count: 27,
      fresh_processes: true,
      summary: ["median", "p95", "p99"],
      seed: "tondo-native-aot-memory-0.1"
  }
  and .native_aot_memory.vm.status == "semantics-only-oracle"
  and .native_aot_memory.vm.counters == "not-comparable-implementation-observation"
  and ([.native_aot_memory.candidates[] | select(.status == "passed" and .instrumented == true and .semantic_equivalence == "all-admitted-cases-and-traps-checked-before-counters" and (.samples | length == 27))] | length == 2)
  and all(.native_aot_memory.candidates[];
      (.product_sha256 | test("^sha256:[0-9a-f]{64}$"))
      and ([.samples[] | select(.duration_ns > 0 and .allocation_count > 0 and .allocated_bytes > 0 and .peak_live_bytes > 0 and .live_bytes == 0 and .retain_local > 0 and .retain_atomic > 0 and .release_local > 0 and .release_atomic > 0 and .cycles_reclaimed > 0 and .weak_upgrades > 0 and .pause_ns > 0 and .concurrency_operations > 0 and .rss_peak_bytes > 0 and .allocated_bytes >= .peak_live_bytes)] | length == 27)
      and ([.samples[].process] | unique | sort) == [0, 1, 2]
      and ((.samples | group_by(.process) | map(length)) == [9, 9, 9])
      and (.summary | keys_unsorted) == [
        "allocation_count", "allocated_bytes", "peak_live_bytes", "live_bytes",
        "retain_local", "retain_atomic", "release_local", "release_atomic",
        "cycles_reclaimed", "weak_upgrades", "pause_ns",
        "concurrency_operations", "rss_peak_bytes"
      ]
      and all(.summary[]; .median >= 0 and .p95 >= .median and .p99 >= .p95)
      and (.summary.live_bytes == {median: 0, p95: 0, p99: 0})
  )
' "$report" >/dev/null || die "runner report did not prove native AOT memory evidence"

! grep -Fq "$root" "$report" || die "runner report leaked a physical workspace path"
echo "native evaluation runner: PASS (Cranelift/LLVM scalar, runtime, AOT-lowering and linked-product executables; report: ${report#"$root"/})"
