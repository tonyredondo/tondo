#!/bin/sh
# Installs `tondo` from this repository and sets it up for the coding
# agents of this machine: builds it, or without Rust downloads the latest
# build of main that passed CI, puts it in a directory of programs,
# checks that it compiles and runs a program, installs the Tondo skill
# and registers `tondo mcp` with Claude Code and Codex where they are
# installed. Running it again updates all of it.
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
say "installed $tondo, $("$tondo" version)"

printf 'import std.console\n\nfn main() {\n    console.println("hello from tondo")\n}\n' >"$work/hello.to"
ran=$("$tondo" run "$work/hello.to" 2>&1) || true
[ "$ran" = "hello from tondo" ] || fail "tondo is installed, but running a program gave: $ran"
say "it compiles and runs a program"

request='{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28"}}}'
case $(printf '%s\n' "$request" | "$tondo" mcp 2>&1) in
*'"supportedVersions"'*) say "its MCP server answers" ;;
*) fail "tondo mcp did not answer; run \`tondo mcp\` and send it: $request" ;;
esac

skill() {
    mkdir -p "$1/tondo"
    "$tondo" guide --skill >"$1/tondo/SKILL.md"
    say "$2: the skill is in $1/tondo/SKILL.md"
}
agents=""
if command -v claude >/dev/null || [ -d "$HOME/.claude" ]; then
    skill "$HOME/.claude/skills" "Claude Code"
    agents="Claude Code"
fi
if command -v codex >/dev/null || [ -d "$HOME/.codex" ]; then
    # Where Codex and the other agents of the Agent Skills standard look.
    skill "$HOME/.agents/skills" "Codex"
    agents="${agents:+$agents and }Codex"
fi
if command -v claude >/dev/null; then
    claude mcp remove --scope user tondo >/dev/null 2>&1 || true
    claude mcp add --scope user tondo -- "$tondo" mcp >/dev/null
    say "Claude Code: tondo mcp serves every project"
fi
if command -v codex >/dev/null; then
    codex mcp remove tondo >/dev/null 2>&1 || true
    codex mcp add tondo -- "$tondo" mcp >/dev/null
    say "Codex: tondo mcp serves every project"
fi

case ":$PATH:" in
*":$bin:"*) ;;
*) say "$bin is not on PATH: add it, as export PATH=\"$bin:\$PATH\" in your shell's profile" ;;
esac
if [ -n "$agents" ]; then
    say "done: start a new session of $agents to load the skill and the MCP server"
else
    say "done; for an agent, \`tondo guide --skill\` prints the skill, and \`$tondo mcp\` is the MCP server"
fi
say "new to Tondo? \`tondo guide\`"
