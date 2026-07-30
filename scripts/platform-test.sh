#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

bash scripts/materialize-checkpoint-spec.sh --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo build -p tondo-cli --locked

binary="target/debug/tondo"
if [[ "${RUNNER_OS:-}" == "Windows" ]]; then
    binary="target/debug/tondo.exe"
fi

"$binary" --version
output="$("$binary" run tests/runtime/g2-002-hello-world.to)"
printf 'Tondo output: %s\n' "$output"
test "$output" = "Hello, world"
