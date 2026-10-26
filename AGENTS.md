# Tondo, for agents

This repository is Tondo, a statically typed language compiled to native
executables, and `tondo`, its whole toolchain in one binary: compiler,
runtime, standard library, test runner, formatter, language server, MCP
server and debugger. This file says how to set Tondo up for a user, how
to write Tondo, and how to work on the compiler.

## Set Tondo up

When the user wants to use Tondo, install it, check it and hand it over,
in this order.

1. **Install.** On Linux or macOS, from the root of this repository:

   ```sh
   sh dist/install.sh
   ```

   It builds `tondo` with Rust and installs it in `~/.cargo/bin`; without
   Rust, it downloads the latest build of `main` that passed CI, with
   GitHub's `gh` signed in, and installs it in `~/.local/bin`; or in the
   directory given as its argument. Then it runs `tondo setup`, which
   checks that `tondo` compiles and runs a program and that its MCP server
   answers, writes the Tondo skill for Claude Code
   (`~/.claude/skills/tondo`) and for Codex and the other agents of the
   Agent Skills standard (`~/.agents/skills/tondo`), registers
   `tondo mcp` with Claude Code and Codex for every project, for those of
   them that are installed, and installs the Tondo extension in VS Code,
   VS Code Insiders, Cursor, VSCodium and Windsurf, for those that are
   installed: the extension of the latest build of `main` that passed CI,
   through `gh`, or without `gh` one that npm makes from this repository.
   Each line printed says what was done; when it stops, its last line says
   what is missing and how to get it. Running it again updates everything.

   With a `tondo` already installed, as one downloaded from CI,
   `tondo setup` alone does the rest. By hand, when the script cannot run:

   ```sh
   cargo build --release --locked -p tondo     # Rust from https://rustup.rs, and a C compiler
   cp target/release/tondo ~/.cargo/bin/
   (cd editors/vscode && npm ci && npm run package -- -o /tmp/tondo.vsix)   # Node from https://nodejs.org
   tondo setup --vsix /tmp/tondo.vsix
   ```

   An editor of the VS Code family that `tondo setup` does not find, as one
   whose command is not on `PATH`, takes the `.vsix` with
   `<editor> --install-extension <file>`.

   Any other MCP client runs it over standard input and output:
   `{ "mcpServers": { "tondo": { "command": "tondo", "args": ["mcp"] } } }`.

2. **Check.**

   ```sh
   tondo version                     # tondo 0.1.0-dev (native, cranelift)
   printf 'import std.console\n\nfn main() {\n    console.println("hello")\n}\n' > /tmp/hello.to
   tondo run /tmp/hello.to           # hello
   claude mcp list                   # tondo: ... mcp - ✔ Connected
   codex mcp list                    # tondo ... enabled
   code --list-extensions | grep tondo   # tondo.tondo, and the same with cursor, codium…
   tondo setup --json                # what is set up, as data
   ```

   `tondo: command not found` means its directory is not on `PATH`: the
   script said which one to add to the shell's profile.

3. **Hand it over.** The skill and the MCP server load when a session of
   the agent starts: tell the user to start a new one. In it, the tools of
   `tondo` are there: `check`, `fix`, `run`, `test`, `debug_start` and the
   rest. An editor that was open loads the extension when its windows
   reload (*Developer: Reload Window*); *Tondo: Get Started* walks through
   it, and *Tondo: New Project* makes a project.

## Write Tondo

Tondo is new, and what is true of other languages often is not of it.

- Read `tondo guide` before writing it, then the topics it names, and
  `tondo guide pitfalls` before writing much. `tondo api <module>` shows
  what a module offers; `tondo api --list` names the modules.
- After every edit, check: the MCP tool `check`, or `tondo check --json`.
  Each diagnostic says what to write; the tool `fix`, or the `fixes` of
  the JSON, applies the safe repairs.
- Run with `run`, test with `test` and format with `format`, as tools or
  as `tondo run`, `tondo test` and `tondo fmt`.
- With the MCP server, prefer its tools: they give data, take a program's
  input, end a program that runs too long, and debug.
- A program that keeps running, as a server, is a `run_start` session: after
  an edit, `reload` changes its code while it runs and keeps its state,
  instead of starting it again (`tondo run --watch` outside MCP).

## Work on the compiler

In this repository, `cargo run -q -p tondo -- <command>` is the `tondo` of
the working tree. `README.md` says where everything is; the
specifications in `spec/` define what Tondo is.

Before every commit, as CI does on Linux, macOS, musl and under emulation:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
TONDO_CASES_DEBUG=1 cargo test -p tondo --test cases
```

- The implementation follows the specifications. Changing one is the
  maintainer's decision: propose it, with a recommendation, before
  writing it.
- A feature lands with its tests and its documents: a program in
  `tests/cases` whose comments state its output, exit, panic or
  diagnostic; the section of the specification; the guide's topic, in
  `crates/tondo/guide`, when users meet it; an entry in
  `docs/decisions.md` for a decision of design; and its row in the
  README's roadmap.
- Errors say how to fix them, with the code to write when there is one.
  The toolchain does what it can by itself rather than ask for
  annotations.
- Keep it small: no code a feature does not need.
- The skill in `.agents/skills/tondo`, which `.claude/skills/tondo` links
  to, is `tondo guide --skill`: after changing the guide's overview, write
  it again with
  `cargo run -q -p tondo -- guide --skill > .agents/skills/tondo/SKILL.md`.
  A test checks it.
- Commits have a short title that says what changed and a body that says
  why, with no trailers.
