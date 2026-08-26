#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
scripts/native-abi-check.sh
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" cargo test -p tondo-compiler --lib toolchain::tests::native_memory_and_abi --locked
echo "native ABI contract tests: PASS"
