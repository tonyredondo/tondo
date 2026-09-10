#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-fuzz-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib fuzz: $name unexpectedly passed" >&2
        exit 1
    fi
}

scripts/stdlib-fuzz-check.sh >/dev/null
scripts/stdlib-owner-evidence-check.sh >/dev/null

# These are reviewed classifications of the source bound by audited_sources,
# not an inference from the presence of a target or seed file.
jq -e '
  def owners($kind): [.owners[] | select(.evidence_kind == $kind) | .id] | sort;
  owners("constant-admission") == ["std.async", "std.console", "std.core", "std.env", "std.fs", "std.iter", "std.process", "std.time"]
  and owners("rust-reference-only") == ["std.bytes", "std.collections", "std.text"]
  and owners("kernel-smoke") == []
  and owners("kernel-invariants") == ["std.format", "std.io", "std.json", "std.math", "std.messagepack", "std.path", "std.protobuf", "std.serialization", "std.testing"]
  and owners("bounded-model") == ["std.meta", "std.reflect"]
' testing/stdlib-fuzz.json >/dev/null

jq '.owners = .owners[1:]' testing/stdlib-fuzz.json > "$tmp_dir/missing-owner.json"
expect_failure missing-owner env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/missing-owner.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].corpus = "fuzz/corpus/stdlib_owners/missing/seed"' testing/stdlib-fuzz.json > "$tmp_dir/missing-corpus.json"
expect_failure missing-corpus env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/missing-corpus.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].route = "std.unknown"' testing/stdlib-fuzz.json > "$tmp_dir/unknown-route.json"
expect_failure unknown-route env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/unknown-route.json" scripts/stdlib-fuzz-check.sh

jq '.status = "promoted" | .owners[].status = "verified" | .owners[].reason = null' \
    testing/stdlib-fuzz.json > "$tmp_dir/false-promotion.json"
expect_failure false-promotion env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/false-promotion.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].component_status = "verified"' testing/stdlib-fuzz.json > "$tmp_dir/admission-as-kernel.json"
expect_failure admission-as-kernel env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/admission-as-kernel.json" scripts/stdlib-fuzz-check.sh

jq '.audited_sources[0].sha256 = ("0" * 64)' testing/stdlib-fuzz.json > "$tmp_dir/stale-source.json"
expect_failure stale-source env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/stale-source.json" scripts/stdlib-fuzz-check.sh

jq '.owners[0].cells.FUZZ.status = "verified" | .owners[0].cells.FUZZ.reason = null' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/false-owner.json"
expect_failure false-owner env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/false-owner.json" scripts/stdlib-owner-evidence-check.sh

# Select an explicitly unobserved owner: array position zero can be an owner
# whose public conformance has already been verified.
jq -e 'any(.owners[]; .id == "std.console" and .cells.CONF.status == "pending")' \
    testing/stdlib-owner-evidence.json >/dev/null
jq '(.owners[] | select(.id == "std.console")).cells.CONF |= (.status = "verified" | .reason = null)' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/declared-as-observed.json"
expect_failure declared-as-observed env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/declared-as-observed.json" scripts/stdlib-owner-evidence-check.sh

jq '.owners[0].cells.SPEC.status = "unknown" | .owners[0].cells.SPEC.reason = "unsupported state"' \
    testing/stdlib-owner-evidence.json > "$tmp_dir/unknown-state.json"
expect_failure unknown-state env TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/unknown-state.json" scripts/stdlib-owner-evidence-check.sh

scripts/stdlib-owner-evidence-generate.sh "$tmp_dir/generated.json" >/dev/null
cmp -s "$tmp_dir/generated.json" testing/stdlib-owner-evidence.json
TONDO_STDLIB_OWNER_EVIDENCE="$tmp_dir/generated.json" \
    scripts/stdlib-fuzz-promote-evidence.sh "$tmp_dir/generated-again.json" >/dev/null
cmp -s "$tmp_dir/generated.json" "$tmp_dir/generated-again.json"

printf 'prior output\n' > "$tmp_dir/preserved.json"
cp "$tmp_dir/preserved.json" "$tmp_dir/prior.json"
expect_failure invalid-generation env TONDO_STDLIB_FUZZ_CONTRACT="$tmp_dir/false-promotion.json" \
    scripts/stdlib-owner-evidence-generate.sh "$tmp_dir/preserved.json"
cmp -s "$tmp_dir/preserved.json" "$tmp_dir/prior.json"

echo "stdlib fuzz tests: OK"
