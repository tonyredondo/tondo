#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="${TONDO_NATIVE_AOT_QUALITY_TMPDIR:-$root/.tmp}"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-quality-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_contract_failure() {
    local name="$1" candidate="$2"
    if TONDO_NATIVE_AOT_QUALITY_CONTRACT="$candidate" scripts/native-aot-quality-check.sh >/dev/null 2>&1; then
        echo "native AOT quality tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

for mutation in \
    '(.required_evidence.candidate_status = "one-passed")' \
    '(.input.oracle = "mutated")' \
    '(.required_evidence.divergences = 1)' \
    '(.protocol.native_runtime_cases = 20)' \
    '(.protocol.native_diagnostic_cases = 7)' \
    '(.protocol.owner_fuzz_targets = 4)' \
    '(.protocol.sanitizers = ["address"])' \
    '(.required_evidence.unsupported_admitted = 1)' \
    '(.required_evidence.physical_paths = "allowed")' \
    '(.required_evidence.mutation = "partial")' \
    '(.next_blocks = ["DEC-013"])' \
    '(.mutation_oracles = .mutation_oracles[1:])'; do
    name="mutation-${#mutation}"
    jq "$mutation" testing/native-aot-quality.json > "$tmp/$name.json"
    expect_contract_failure "$name" "$tmp/$name.json"
done

revision="$(printf '0%.0s' {1..40})"
hash="$(printf '0%.0s' {1..64})"
baseline_basis_points="$(jq -r '.coverage.global.lines.basis_points' testing/quality-baseline.json)"
cat > "$tmp/valid-report.json" <<EOF
{
  "format": "tondo-native-aot-quality/1",
  "task": "NATIVE-AOT-QUALITY-001",
  "phase": "NATIVE-AOT-QUALITY-001",
  "status": "passed",
  "candidates": ["cranelift", "llvm"],
  "oracle": {"vm":"bytecode-vm-oracle","mir":"normalized-MIR-reference-interpreter","mismatch":"fail-closed"},
  "native": {"status":"passed","candidate_status":"both-passed","unsupported_admitted":0,"divergences":0,"case_counts":{"native_aot_lowering":27,"native_scalar":118,"native_managed":3,"native_runtime":21,"native_std_core":14,"native_diagnostics":8},"fields":["native_aot_binary","native_aot_lowering","native_aot_memory","native_diagnostics"]},
  "conformance": {"status":"passed","categories":{"language":{"status":"passed","cases":3},"testing":{"status":"passed","cases":3},"stdlib":{"status":"passed","cases":3}},"cases":9},
  "differential": {"status":"passed","cases":9,"backends":["cranelift","llvm"],"oracle":"bytecode-vm-oracle","cross_backend_equality":true,"stable_generation":true,"fail_closed_mutation":true},
  "fuzz": {"owner":{"status":"passed","targets":5,"runs_per_target":128,"max_input_bytes":65536,"timeout_seconds":10,"rss_limit_mb":4096,"bounded":true,"deterministic":true,"regressions_replayed":true},"diagnostics":{"status":"passed","targets":1,"runs":128,"max_input_bytes":65536,"timeout_seconds":10,"rss_limit_mb":4096,"bounded":true,"deterministic":true,"regressions_replayed":true}},
  "sanitizers": {"status":"passed","compiler":"explicit-cc-sanitizer-wrapper","address":"passed","undefined":"passed","fresh_processes":true},
  "workspace_quality": {"status":"passed","baseline_unchanged":true,"baseline_basis_points":$baseline_basis_points,"mutation":"passed","mutation_sample":{"status":"passed","total":6,"caught":6,"missed":0,"timeout":0,"unviable":0,"score_basis_points":10000,"selection":"one-per-critical-frontier"},"baseline_sha256":"sha256:$hash"},
  "mutation": {"status":"passed","oracles":12,"rejected":12},
  "physical_paths": [], "divergences": [], "unsupported": [],
  "source_revision": "$revision", "contract_sha256":"sha256:$hash", "native_report_sha256":"sha256:$hash", "native_aot_sanitized_report_sha256":"sha256:$hash",
  "touched_files": ["scripts/native-aot-quality.sh"]
}
EOF

expect_report_failure() {
    local name="$1" candidate="$2"
    if TONDO_NATIVE_AOT_QUALITY_REPORT="$candidate" scripts/native-aot-quality-check.sh >/dev/null 2>&1; then
        echo "native AOT quality tests: report $name unexpectedly passed" >&2
        exit 1
    fi
}

for mutation in \
    '(.native.candidate_status = "one-passed")' \
    '(.oracle.vm = "mutated")' \
    '(.differential.cross_backend_equality = false)' \
    '(.native.case_counts.native_scalar = 117)' \
    '(.fuzz.diagnostics.status = "failed")' \
    '(.sanitizers.address = "failed")' \
    '(.workspace_quality.baseline_unchanged = false)' \
    '(.workspace_quality.baseline_basis_points = 0)' \
    '(.mutation.rejected = 11)' \
    '(.physical_paths = ["/tmp/private"])' \
    '(.unsupported = ["admitted-case"])' \
    '(.source_revision = "stale")'; do
    name="report-${#mutation}"
    jq "$mutation" "$tmp/valid-report.json" > "$tmp/$name.json"
    expect_report_failure "$name" "$tmp/$name.json"
done

scripts/native-aot-quality-check.sh >/dev/null
grep -Fq 'fsanitize=address,undefined' scripts/native-aot-sanitize-cc.sh
grep -Fq 'stable_source="/tmp/tondo-native-aot-sanitized.c"' scripts/native-aot-sanitize-cc.sh
grep -Fq 'fno-sanitize=integer-divide-by-zero' scripts/native-aot-sanitize-cc.sh
grep -Fq -- '--gitignore true' scripts/quality-gate.sh
grep -Fq -- '--error ' scripts/quality-gate.sh
grep -Fq -- '--jobs 1' scripts/quality-gate.sh
grep -Fq -- 'one-per-frontier' scripts/quality-gate.sh
grep -Fq -- '--no-shuffle' scripts/quality-gate.sh
echo "native AOT quality tests: OK (12 contract and 11 report oracle mutations rejected)"
