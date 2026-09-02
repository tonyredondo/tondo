#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_CHANNEL_TEST_CONTRACT:-$root/testing/stdlib-channel-test.json}"

die() {
    echo "std.channel tests: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing testing contract: ${contract#"$root"/}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "testing contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "testing contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-channel-testing/1"
  and .owner == "std.channel"
  and .parent_owner == "std.async"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-CHANNEL-TEST-001"
  and .status == "verified"
  and .contract == "docs/contracts/stdlib-channel-test.md"
  and .implementation_contract == "testing/stdlib-channel.json"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .parent_contract == "testing/stdlib-channel.json"
  and .layer == "B2"
  and .kind == "reliability-facing"
  and .target == "reference-model-and-hosted-native-regression-boundary"
  and .limits.max_channel_buffer == 64
  and .limits.max_unbounded_queue == 64
  and .limits.max_channel_handles == 128
  and .limits.max_fuzz_input_bytes == 4096
  and .limits.max_fuzz_steps == 512
  and .limits.model_seed_count == 4096
  and .limits.fuzz_smoke_runs == 128
  and .model.status == "verified"
  and (.model.sources | type == "array" and length == 2)
  and (.model.laws | type == "array" and length >= 16)
  and .model.sequence_seeds == 4096
  and .model.oracle == "independent bounded channel state, affine payload ledger, FIFO waiter queues and exact wakeup accounting"
  and .model.command == "cargo test -p tondo-reliability --test channel_models --locked"
  and .test.status == "verified"
  and (.test.sources | type == "array" and length == 5)
  and (.test.commands | type == "array" and length == 5)
  and (.test.cases | type == "array" and length >= 12)
  and .test.oracle == "runtime regression suites and independent model observations agree on ownership, FIFO, wakeup and cleanup outcomes"
  and .fuzz.status == "verified"
  and .fuzz.target == "stdlib_channel"
  and .fuzz.source == "fuzz/fuzz_targets/stdlib_channel.rs"
  and .fuzz.corpus == "fuzz/corpus/stdlib_channel/seed"
  and .fuzz.input_limit_bytes == 4096
  and .fuzz.step_limit == 512
  and .fuzz.smoke.runs == 128
  and .fuzz.smoke.seed == 4104
  and .fuzz.smoke.result == "passed"
  and .fuzz.smoke.toolchain == "nightly-2026-07-28"
  and .fuzz.oracle == "panic-free bounded replay, affine ownership invariants, exact wakeup accounting and structured cleanup"
  and .fuzz.command == "TONDO_CHANNEL_FUZZ_RUNS=128 scripts/stdlib-channel-fuzz.sh"
  and .sanitization.status == "bounded-safe-rust-no-unsafe-boundary"
  and .sanitization.applicable == false
  and .sanitization.native_aot == "not-claimed"
  and .promotion.model_test_fuzz_complete == true
  and .promotion.implementation_pending == []
  and .promotion.next_blocks == ["STD-CHANNEL-DOC-001"]
  and .promotion.remaining == []
' "$contract" >/dev/null || die "invalid machine-readable channel testing contract"

for path in \
    docs/contracts/stdlib-channel-test.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_LANGUAGE_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md \
    testing/stdlib-channel.json \
    testing/inventory.json \
    testing/coverage-matrix.json; do
    [[ -f "$root/$path" ]] || die "missing linked path: $path"
done

while IFS= read -r path; do
    [[ -f "$root/$path" ]] || die "missing model/test source: $path"
done < <(jq -r '.model.sources[], .test.sources[], .fuzz.source, .fuzz.corpus' "$contract")

for path in \
    scripts/stdlib-channel-test-check.sh \
    scripts/stdlib-channel-test-test.sh \
    scripts/stdlib-channel-fuzz.sh; do
    [[ -x "$root/$path" ]] || die "runner is not executable: $path"
done

grep -Fq 'name = "stdlib_channel"' "$root/fuzz/Cargo.toml" \
    || die "fuzz manifest misses stdlib_channel"

for marker in \
    'MAX_CHANNEL_BUFFER' \
    'MAX_UNBOUNDED_QUEUE' \
    'MAX_CHANNEL_HANDLES' \
    'MAX_CHANNEL_FUZZ_INPUT_BYTES' \
    'MAX_CHANNEL_FUZZ_STEPS' \
    'pub enum ChannelCapacity' \
    'pub enum ChannelModelError' \
    'pub enum SelectArm' \
    'pub struct ChannelModel' \
    'pub fn assert_invariants' \
    'pub fn cleanup' \
    'pub fn run_fuzz_case'; do
    grep -Fq "$marker" "$root/crates/tondo-reliability/src/channel_model.rs" \
        || die "model misses anchor: $marker"
done

for marker in \
    'std.channel model panicked' \
    'std.channel model invariant failed' \
    'std.channel model replay diverged' \
    'MAX_CHANNEL_FUZZ_INPUT_BYTES'; do
    grep -Fq "$marker" "$root/fuzz/fuzz_targets/stdlib_channel.rs" \
        || die "fuzz target misses anchor: $marker"
done

jq -e '
  .testing == "testing/stdlib-channel-test.json"
  and .promotion.next_blocks == ["STD-CHANNEL-DOC-001"]
  and .promotion.implementation_pending == [
    "STD-CHANNEL-DOC-001"
  ]
  and .implementation.required_follow_ups == .promotion.implementation_pending
' "$root/testing/stdlib-channel.json" >/dev/null \
    || die "parent channel registry does not expose the promoted testing boundary"

grep -Fq 'stdlib-channel-test.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" \
    || die "stdlib spec does not link the channel testing contract"
grep -Fq 'stdlib-channel-test.md' "$root/docs/contracts/stdlib-channel.md" \
    || die "channel document does not link the channel testing contract"
grep -Fq 'STD-CHANNEL-TEST-001' "$root/TONDO_IMPLEMENTATION_TRACKER.md" \
    || die "tracker does not record the channel testing leaf"

echo "std.channel tests: OK (independent model; deterministic replay; hosted/native regressions; bounded fuzz)"
