#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-regex-implementation.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
expect_failure() {
    local name="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "std.regex implementation tests: $name unexpectedly passed" >&2; exit 1
    fi
}
for transition in ready verified; do
    if [[ "$transition" == ready ]]; then
        jq '.implementation.status = "ready-stdlib-kernel" | .implementation.quality_gate = "pending-80-percent-per-scope" | .promotion.next_blocks = ["STD-REGEX-IMPL-001"] | .implementation.required_follow_ups = ["STD-REGEX-IMPL-001", "STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]' testing/stdlib-regex.json > "$tmp_dir/base.json"
    else
        jq '.implementation.status = "verified-stdlib-kernel" | .implementation.quality_gate = "verified-80-percent-per-scope" | .promotion.next_blocks = ["STD-REGEX-TEST-001"] | .implementation.required_follow_ups = ["STD-REGEX-TEST-001", "STD-REGEX-PERF-001", "STD-REGEX-CONF-001", "STD-REGEX-DOC-001"]' testing/stdlib-regex.json > "$tmp_dir/base.json"
    fi
    for checker in scripts/stdlib-regex-check.sh scripts/stdlib-regex-implementation-check.sh; do
        TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/base.json" bash "$checker"
        for mutation in \
            '.implementation.status = "pending-after-native-gate"' \
            '.implementation.quality_gate = "passed"' \
            '.implementation.public_api_promoted = true' \
            '.implementation.production_vm_registration = "verified"' \
            '.implementation.native_abi = "verified"' \
            '.implementation.native_aot_lowering = "verified"' \
            '.implementation.parser_dependency.version = "0.8"' \
            '.implementation.sources |= .[0:2]' \
            '.implementation.tests |= .[0:9]' \
            '.api.iterator_protocol = "Iterator[RegexMatch]"' \
            '.api.terminal_state = "iterator-none-is-terminal"' \
            '.semantics.tie_break = "greedy-longest-then-alternative-order"' \
            '.errors.iterator_atomicity = "whole-cursor"' \
            '.promotion.next_blocks = ["STD-REGEX-DOC-001"]'; do
            jq "$mutation" "$tmp_dir/base.json" > "$tmp_dir/invalid.json"
            expect_failure "$transition/$mutation" env TONDO_STDLIB_REGEX_CONTRACT="$tmp_dir/invalid.json" bash "$checker"
        done
    done
done
cargo test -q -p tondo-stdlib --locked --test regex_kernel
echo "std.regex implementation tests: OK (kernel behavior; per-element errors; both promotion transitions)"
