#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target="${TONDO_TEST_TARGET:-host-native}"
report="${TONDO_NATIVE_PORTABILITY_REPORT:-target/platform-test/${target}/native-portability.json}"
target_dir="${CARGO_TARGET_DIR:-target}"

die() {
    echo "native portability: $*" >&2
    exit 1
}

expected_triple=""
expected_format=""
case "$target" in
    linux-x86_64) expected_triple="x86_64-unknown-linux-gnu"; expected_format="elf" ;;
    linux-aarch64) expected_triple="aarch64-unknown-linux-gnu"; expected_format="elf" ;;
    macos-x86_64) expected_triple="x86_64-apple-darwin"; expected_format="macho" ;;
    macos-aarch64) expected_triple="aarch64-apple-darwin"; expected_format="macho" ;;
    windows-x86_64) expected_triple="x86_64-pc-windows-msvc"; expected_format="coff" ;;
    host-native) expected_triple="$(rustc -vV | sed -n 's/^host: //p')" ;;
    *) die "unknown platform id: $target" ;;
esac

host_triple="$(rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host_triple" ]] || die "rustc did not report a host triple"
[[ "$host_triple" == "$expected_triple" ]] \
    || die "runner host is $host_triple, expected $expected_triple for $target"

mkdir -p "$(dirname "$report")"

CARGO_TARGET_DIR="$target_dir" cargo test \
    --manifest-path tools/native-evaluation/Cargo.toml --locked --quiet

probe_json="$(CARGO_TARGET_DIR="$target_dir" cargo run \
    --manifest-path tools/native-evaluation/Cargo.toml --locked --quiet \
    --bin cranelift-portability -- --target "$host_triple")"

printf '%s\n' "$probe_json" > "$report"

jq -e \
    --arg target "$host_triple" \
    --arg object_format "$expected_format" \
    '.format == "tondo-native-portability-probe/1"
     and .status == "passed"
     and .backend == "cranelift"
     and .cranelift_version == "0.132.3"
     and .target == $target
     and .object_format == $object_format
     and (.object_bytes | type == "number" and . > 0)
     and (.architecture | test("^[a-z0-9_]+$"))
     and (.os | test("^[a-z0-9]+$"))' "$report" >/dev/null \
    || die "probe report failed validation"

! grep -Fq "$root" "$report" || die "probe report leaked a physical path"

echo "native portability: PASS (Cranelift ${host_triple}/${expected_format}; report: ${report#"$root/"})"
