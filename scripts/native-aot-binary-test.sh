#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
tmp_root="$root/.tmp"
mkdir -p "$tmp_root"
tmp="$(mktemp -d "$tmp_root/tondo-native-aot-binary-test.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT

expect_failure() {
    local name="$1"
    local candidate="$2"
    if TONDO_NATIVE_AOT_BINARY_CONTRACT="$candidate" \
        scripts/native-aot-binary-check.sh >/dev/null 2>&1; then
        echo "native AOT binary tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.status = "evaluation-ready"' testing/native-aot-binary.json > "$tmp/open.json"
expect_failure status "$tmp/open.json"

jq '.input.fresh_builds_per_candidate = 1' testing/native-aot-binary.json > "$tmp/one-build.json"
expect_failure one-build "$tmp/one-build.json"

jq '.input.same_target_runtime_stdlib_linker_profile = false' \
    testing/native-aot-binary.json > "$tmp/different-inputs.json"
expect_failure different-inputs "$tmp/different-inputs.json"

jq '.product.required_nonempty_section = ".code"' \
    testing/native-aot-binary.json > "$tmp/missing-text.json"
expect_failure missing-text "$tmp/missing-text.json"

jq '.identity.physical_paths = "allowed"' testing/native-aot-binary.json > "$tmp/paths.json"
expect_failure paths "$tmp/paths.json"

jq '.next_blocks = ["DEC-013"]' testing/native-aot-binary.json > "$tmp/stale-next.json"
expect_failure stale-next "$tmp/stale-next.json"

scripts/native-aot-binary-check.sh >/dev/null
grep -Fq 'two fresh builds' docs/contracts/native-aot-binary.md
grep -Fq 'strip --strip-debug' docs/contracts/native-aot-binary.md
grep -Fq 'physical path' docs/contracts/native-aot-binary.md

echo "native AOT binary tests: OK (contract drift and non-comparable product evidence rejected)"
