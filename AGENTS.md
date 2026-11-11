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

While working on a feature, `TONDO_RUNTIME_FAST=1` in the environment of
`cargo` compiles a change to the runtime in about half the time. Leave it
out for the final check below, which is the build that ships.

The runtime is built in `target/tondo-runtime`, under a key of what makes
it (`crates/tondo/build.rs`): every way of compiling `tondo` takes it from
there, and only a change to the runtime, its dependencies or the
toolchains builds it again.

Before every commit, as CI does on Linux, macOS, musl and under emulation:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
TONDO_CASES_DEBUG=1 cargo test --workspace --test cases
TONDO_CASES_REPLAY=1 cargo test --workspace --test cases cases
```

`.github/scripts/local.sh` runs what CI and the Nightly add, in containers
on this machine: the servers, no C compiler, musl, emulation, and the tests that
race with little CPU, which is where a failure otherwise shows hours later.
The tree stays as it is while these checks run: bash reads a script as it
runs it, and cargo builds what is there.

A newer push to a branch ends CI's run of the one before, which is
superseded, not failed. A change to the workflows is tried on a branch of
its own, with `gh workflow run CI --ref <branch>`.

- The implementation follows the specifications. Changing one is the
  maintainer's decision: propose it, with a recommendation, before
  writing it.
- A feature lands with its tests and its documents: a program in
  `tests/cases` whose comments state its output, exit, panic or
  diagnostic, and for an optimization what a release build makes of the
  code (`// expect-optimization:`); the section of the specification; the guide's topic, in
  `crates/tondo/guide`, when users meet it; an entry in
  `docs/decisions.md` for a decision of design; and its row in
  `docs/roadmap.md`, and in the README's status when it changes it.
- Errors say how to fix them, with the code to write when there is one.
  The toolchain does what it can by itself rather than ask for
  annotations.
- Keep it small: no code a feature does not need.
- The public API of the standard library is recorded in
  `crates/tondo/tests/snapshots/std-api.txt`, which a test holds it to: a
  change to it on purpose records it again with
  `TONDO_UPDATE_API=1 cargo test -p tondo --test api standard_api`.
- The formats others read, the report of a panic, a dump and the report of
  `tondo test`, are recorded in `crates/tondo/tests/snapshots/formats`: a
  change on purpose records them again with
  `TONDO_UPDATE_FORMATS=1 cargo test -p tondo --test formats`.
- The skill in `.agents/skills/tondo`, which `.claude/skills/tondo` links
  to, is `tondo guide --skill`: after changing the guide's overview, write
  it again with
  `cargo run -q -p tondo -- guide --skill > .agents/skills/tondo/SKILL.md`.
  A test checks it.
- Commits have a short title that says what changed and a body that says
  why, with no trailers.

### See what the compiler does

Ask the toolchain before guessing why code is slow or allocates:

- `tondo optimizations [<path>]`, or the MCP tool `optimizations`, says
  what a release build made of each place of a program: the calls it
  inlined and those it kept, with why; the texts and closures made in the
  frame, and what keeps the others on the heap; the checks dropped, and
  those a loop keeps. `--kind heap` gives only what allocates, `--function
  <name>` one function, and `--against <commit>` only what a change made
  different. A pass that decides something the author of the
  code could change says so there, with `mir_text::remark`.
- `tondo asm <function>`, or the MCP tool `asm`, gives the machine code a
  release build makes of a function under the lines it is of, with what
  `tondo optimizations` says of each; `--clif` adds Cranelift's IR before
  and after it optimizes, where a `Tondo:` rule shows whether it applied,
  and `--against <commit>` diffs the code with a revision's.
  `--all --compiler <old tondo>` gives the functions whose code a change
  to the compiler changes, or that it changes none. In a `tondo` built
  with `--features rules`, `--rules` lists the ISLE rules applied to it,
  Tondo's marked; check a change to `vendor/` with `cargo test -p tondo
  --features rules --test cli the_rules` too.
- `TONDO_DUMP_MIR=<name>` in the environment of a build writes the MIR of
  every function whose name holds `<name>` after lowering and after each
  pass, with what the passes decided and why; `TONDO_DUMP_MIR=<name>@<pass>`
  after that pass alone. Write a note there (`mir_text::note`) when a pass
  decides something one would need to know, rather than a print to take
  out later.
- A debug build of `tondo`, which the tests run, checks the MIR after
  lowering and after each pass (`verify.rs`): a pass that leaves a local
  read on a path that does not assign it fails there, naming the pass and
  the function, with its MIR.
- `tondo test --exec <command>` runs the tests' executable under a
  command: `perf stat`, `valgrind`, `gdb --args`.
- `tondo test --bench` gives, on Linux, the instructions and cycles of a
  run beside its time; `--regression-by instructions` with
  `--max-regression` judges a change by them, which where code lies does
  not move.
- `tondo profile show <profile> --asm [<function>]` gives the instructions
  of the function that took the most, each with its share of the samples:
  which instruction of a hot line the time goes to.
- `tondo test --exact <id> --bench --profile memory` writes where a
  benchmark allocates, every allocation with `TONDO_HEAP_RATE=1` in its
  environment; `tondo profile show <profile> --sample alloc_objects
  --lines` gives the lines that allocate most often.
- `tondo test --bench --layouts <n>` measures each benchmark in n orders of
  the code's functions and gives the range; against a baseline measured so
  too, only a change past every order counts.
- `tondo test bench/tondo --bisect <good> --filter <test> --bench
  --max-regression 5 --regression-by instructions --bisect-with "cargo
  build -q --release -p tondo && echo target/release/tondo"` finds the
  commit of the compiler that made a benchmark do more work.
- A benchmark's row moves by a third when only where its code lies moves.
  Judge a change by the instructions and cycles of a program that does the
  same rounds each time, under `perf stat`, as well as by the row's time;
  `TONDO_LAYOUT_SEED=<n>` in the environment of a build lays its functions
  out in another order, so that a row measured with a few seeds shows how
  much of a difference is where the code lies.
- A change to the passes that should not change the code is proven with
  `tondo asm <program> --all --compiler <the tondo before it>` over the
  programs that run what it touches.

### Where Tondo is going

- As fast as Go or faster, in time and in allocations: every row of
  `bench/`, those of the network too, at or under Go's (`bench/README.md`).
  A change that leaves a row behind Go's needs a reason that outweighs it.
- Cranelift's code generator is Tondo's own copy, in
  `vendor/cranelift-codegen` (`vendor/README.md`): an optimization that
  the generated code lacks is written there, as a rule marked `Tondo:`,
  rather than worked around in the compiler.
- Then a `tondo` written in Tondo, with a version of this one as its base.
  Once the speed and the optimizations are where they should be, the
  standard library gets every API that it needs and lacks.

### Waits and wakes in the runtime

A program that hangs now and then has most often lost a wake. The runtime
holds to these rules:

- Between giving a waker away and blocking on its `Wait`, a task waits
  for nothing else: that wait would take the notification meant for the
  other.
- A waker never polls a future, as the code that wakes it may hold a lock
  the poll takes: it queues it for a worker's turn (`tasks::poll_soon`).
- State that two threads change, as "polled now" and "woken meanwhile",
  is one atomic, changed with read-modify-writes on every side.

`TONDO_WAKES=1` in the environment of a program keeps each task's last
waits and wakes, which `tondo blocked` and a dump show: a task whose wait
was claimed but that is still parked lost the notification; one whose last
wait nothing claimed lost the wake before it.

Such a hang is found under load, as the benchmarks of `bench/tondo` give,
by counting with atomics and printing from a thread of its own what stops
moving: a lock or a print on the path changes the timing that loses the
wake.
