#!/bin/sh
# Installs `tondo` from this repository and sets it up for the coding
# agents of this machine: builds it, or without Rust downloads the latest
# build of main that passed CI, puts it in a directory of programs, and
# runs `tondo setup`, which checks it and gives Claude Code and Codex the
# Tondo skill and `tondo mcp`. Running it again updates all of it.
#
#   sh dist/install.sh [<bin dir>]
#
# The directory is by default Cargo's, ~/.cargo/bin, or without Rust
# ~/.local/bin.
set -eu
cd "$(dirname "$0")/.."
say() { printf 'tondo install: %s\n' "$*"; }
fail() {
    printf 'tondo install: %s\n' "$*" >&2
    exit 1
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if command -v cargo >/dev/null; then
    command -v cc >/dev/null || fail "a C compiler is missing, which Rust links tondo with: install gcc or clang (on macOS, xcode-select --install) and run this again"
    say "building tondo; the first build takes a few minutes"
    cargo build --release --locked -p tondo
    built=target/release/tondo
    bin="${1:-${CARGO_HOME:-$HOME/.cargo}/bin}"
elif command -v gh >/dev/null; then
    case "$(uname -s) $(uname -m)" in
    "Linux x86_64") target=x86_64-linux ;;
    "Linux aarch64" | "Linux arm64") target=aarch64-linux ;;
    "Darwin arm64") target=aarch64-macos ;;
    "Darwin x86_64") target=x86_64-macos ;;
    *) fail "tondo is not built for $(uname -s) on $(uname -m)" ;;
    esac
    case $target in
    *-linux) if ldd --version 2>&1 | grep -qi musl; then target=$target-musl; else target=$target-gnu; fi ;;
    esac
    run=$(gh run list -R tonyredondo/tondo -b main -s success -w CI -L 1 --json databaseId -q '.[0].databaseId') ||
        fail "gh cannot list the builds of tondo: sign in with \`gh auth login\` and run this again"
    say "downloading the latest build of main for $target"
    gh run download "$run" -R tonyredondo/tondo -n "tondo-$target" -D "$work" ||
        fail "gh cannot download the build: sign in with \`gh auth login\` and run this again"
    tar -xzf "$work/tondo-$target.tar.gz" -C "$work"
    built="$work/tondo-$target/bin/tondo"
    bin="${1:-$HOME/.local/bin}"
else
    fail "tondo needs Rust to be built, from rustup (https://rustup.rs), or GitHub's gh (https://cli.github.com) to download a build: install either, open a new shell and run this again"
fi
mkdir -p "$bin"
# Replaced whole, so that a `tondo mcp` running from it goes on.
cp "$built" "$bin/tondo.new"
mv "$bin/tondo.new" "$bin/tondo"
tondo="$bin/tondo"
say "installed $tondo"
"$tondo" setup
