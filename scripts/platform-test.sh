#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo check --workspace --all-targets --locked
cargo build -p tondo-cli --locked
cargo test --workspace --all-targets --locked

binary="target/debug/tondo"
if [[ "${RUNNER_OS:-}" == "Windows" ]]; then
    binary="target/debug/tondo.exe"
fi

"$binary" --version
output="$("$binary" run tests/runtime/g2-002-hello-world.to)"
printf 'Tondo output: %s\n' "$output"
test "$output" = "Hello, world"

platform="${TONDO_TEST_TARGET:-host-native}"
reports="target/platform-test/$platform"
mkdir -p "$reports"
"$binary" test \
    --project acceptance/projects/testing-acceptance \
    --order random \
    --seed 5eed \
    --jobs 2 \
    --report "json=$reports/results.json" \
    --report "junit=$reports/results.xml"
test -s "$reports/results.json"
test -s "$reports/results.xml"
