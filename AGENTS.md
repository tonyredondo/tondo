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

`TONDO_CASES_ONLY=<text>` runs only the cases whose path holds it, in the
mode the other variables of `crates/tondo/tests/cases.rs` choose: one
case in every mode is a loop over them.

A change to the optimizer, the regions or the runtime is measured before it
lands, not after: `tondo test bench/tondo <projects> --bench --compiler HEAD
--baseline bench/baselines/table.json --regression-by instructions
--max-regression 2`, with every project of `examples/` that has a
benchmark, judges it by the benchmarks of single operations and by those of
whole programs, and a row that does over 2% more work lands only with its
reason written in the commit. Each row is also held to the baseline of the
last table against Go, which `--baseline` names there: small costs add up,
and a row that is over 2% past it in all lands only with its reason too,
whoever added what. The baseline is saved again only with a new table.
A row whose cycles grow by over 5% while its instructions stay within the
bar is measured again with `--layouts 4` beside `--compiler HEAD`, each side
in four orders of the code: a growth that stays past every order is the
change's own, and lands only with its reason, as one of instructions does.
Under a row that
moved the benchmarks' report names the code that changed, the runtime's
too, and beside a row the regions its runs set up and how many of them were
used.

A change to the passes that decide who holds a count of a value, `borrowed`,
`moves`, `takes`, `split`, `lends`, `elide` and the regions, runs with the
heap's own checks on before it lands: `.github/scripts/local.sh <out> heap`
builds the runtime with `TONDO_RUNTIME_HEAP_CHECKS=1`, the allocator
checking its own lists, and runs every case, the examples' tests and the
models' ten times. A count dropped where it was needed is a block freed
twice, or read after it was freed, in a program made from safe code, and no
case, order or gate shows it until something walks the heap; a round of the
models catches such a block about once in two, which is why they run ten.
The Nightly runs the same job, and a released `tondo` never has the checks.

`.github/scripts/local.sh` runs what CI and the Nightly add, in containers
on this machine: the servers, no C compiler, musl, emulation, and the tests that
race with little CPU, which is where a failure otherwise shows hours later.
A test that fails now and then is run until it does, in its stage `stress`
with `STRESS_ONLY=<test> STRESS_CPUS=0.25 STRESS_ROUNDS=30`, and its cause
found and fixed rather than the test run again. The tree stays as it is
while these checks run: bash reads a script as it runs it, and cargo builds
what is there.

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
- The guide (`crates/tondo/guide`), which `tondo guide` prints and `tondo
  mcp` serves as its resources and its prompt, its runbooks and the manual
  (`docs/`) say the same thing, and none of them falls behind. A change to
  the language, the standard library, a command or a tool is not done
  until the guide and its runbooks say it, in that same change; the manual
  says it in that change or in the one that follows at once. A change to
  the manual or to the guide is read against the other. What changes how
  a problem is worked, as a tool, an option or a diagnostic, gets its step
  in that problem's runbook, or a runbook of its own. `cargo test -p tondo
  --test runbooks --test guide` holds every tool to a runbook, and every
  topic and runbook to the manual's lists.
- What the command line does, `tondo mcp` and the editor's extension do
  too: a command or an option lands with its tool or its parameter in
  `crates/tondo/src/mcp.rs` and with its command in `editors/vscode`, in
  the same change. `cargo test -p tondo --test guide` holds every command
  and option to both.
- `docs/pending.md` is the one list of what is open and the order it is
  done in: the work follows it. A piece that lands takes its line out in the
  same push; what is found on the way, or agreed in a conversation, gets its
  line that day, checked against the code first; and the order changes
  there, with its reason, never only in a session's notes.
- Errors say how to fix them, with the code to write when there is one.
  The toolchain does what it can by itself rather than ask for
  annotations.
- Keep it small: no code a feature does not need.
- Security and protocol code is used and tuned, not written again here:
  TLS, cryptography and the framing of HTTP stay in their libraries, and an
  optimization changes how they are used. A library that is faster, well
  tested and widely used may replace one; code of our own may not.
- Nothing that reaches an executable takes its order from a hash map's
  iteration: a build is the same bytes twice, as
  `cargo test -p tondo --test cli a_build_is_the_same_bytes_twice` holds.
- The public API of the standard library is recorded in
  `crates/tondo/tests/snapshots/std-api.txt`, which a test holds it to: a
  change to it on purpose records it again with
  `TONDO_UPDATE_API=1 cargo test -p tondo --test api standard_api`.
- The formats others read, the report of a panic, a dump and the report of
  `tondo test`, are recorded in `crates/tondo/tests/snapshots/formats`: a
  change on purpose records them again with
  `TONDO_UPDATE_FORMATS=1 cargo test -p tondo --test formats`.
- The manual in `docs/` is held to the code. The pages of `docs/std` are
  written from `tondo api` and `docs/toolchain/commands.md` from `tondo
  help --all`, and every example of the manual compiles and prints what
  its page says. After a change to the standard library's API or
  documentation, to a command's usage, or to what an example prints, write
  them again with `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs`.
- The programs in `examples/` are formatted, compile without a warning
  and pass their own tests: `cargo test -p tondo --test examples` holds
  them to that.
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
  those a loop keeps; the loops run four turns at a time, the arrays given
  room before the loop that fills them, the elements a loop reads without
  counting them, and the lengths a loop reads once before it, or why not. `--kind heap` gives only what allocates, `--function
  <name>` one function, and `--against <commit>` only what a change made
  different; `--tests` reads the tests and benchmarks instead, as
  `bench/tondo`, whose benchmarks are tests. A pass that decides something
  the author of the code could change says so there, with
  `mir_text::remark`.
- `tondo asm <function>`, or the MCP tool `asm`, gives the machine code a
  release build makes of a function under the lines it is of, with what
  `tondo optimizations` says of each; `--clif` adds Cranelift's IR before
  and after it optimizes, where a `Tondo:` rule shows whether it applied,
  and `--against <commit>` diffs the code with a revision's.
  `--all --compiler <old tondo>` gives the functions whose code a change
  to the compiler changes, the program's with their diffs, then the
  standard library's instances and the glue it reaches, by module, or
  that it changes none; `--all --reached` lists those too, and a piece of
  glue is named by what it does and its type, as release of
  `Map[String, Value]`. In a `tondo` built
  with `--features rules`, `--rules` lists the ISLE rules applied to it,
  Tondo's marked; check a change to `vendor/` with `cargo test -p tondo
  --features rules --test cli the_rules` too.
- `tondo asm --runtime <function>` gives the machine code of a function of
  the runtime, as `tondo` links it, and with `--compiler HEAD` what a
  change to the runtime changed in it: that a hook costs nothing when off
  shows there.
- `--compiler <revision>` in `tondo asm`, `tondo optimizations` and `tondo
  test --bench` compares with the `tondo` of that revision of this
  repository, built once in the cache, as `--compiler HEAD` with what the
  working tree changed: `tondo test bench/tondo --bench --compiler HEAD
  --regression-by instructions --max-regression 2` judges a change to the
  compiler by every benchmark. `tondo compiler <revision>` gives its path,
  for a script, as `bench/compile.to --old`.
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
- `TONDO_CHECK_LIFETIMES=1` in the environment of a program makes the
  memory of each region that ends unreadable, its addresses never given
  again: a value the compiler put in a region it outlives faults where it
  is read, and the report says so. The cases, the examples' tests and
  `bench/tondo`'s tests run with it; a change to what goes in regions is
  tried with it on the programs that run what it touches.
- `tondo test --exec <command>` runs the tests' executable under a
  command: `perf stat`, `valgrind`, `gdb --args`.
- When a row's cycles move with the same instructions and the same code,
  ask the processor why under `tondo test --exec "perf stat -e
  cycles,instructions,…"`: loads that waited on a store,
  `ls_bad_status2.stli_other` on AMD and `ld_blocks.store_forward` on
  Intel, beside cache and branch misses. `perf record -e` of the same event
  names the functions that pay it. Their count bounds what there is to
  gain and does not measure it: a load that waits off the chain of what
  the code computes costs nothing, so a fix is judged by the cycles of a
  program of its own. Each vendor names these events its own way, so the
  bench does not show them.
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
  `tondo asm <program> --all --compiler HEAD` over the programs that run
  what it touches.

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

The scheduler itself is held to its word by four things:

- `TONDO_CHECK_SCHEDULER=1` in the environment of a program makes the runtime
  check as it runs what its scheduler promises (16.14 of the language's
  specification), in looks of its own, and end with `P0020` and a dump at the
  first rule that breaks, with what each worker and the monitor were doing.
  The cases run with it in their debug step, in plain runs: a red there is
  the scheduler's, whatever the case prints. Under a seeded order, a recording
  or a replay it is off, by design.
- A case that holds the scheduler holds no duration, by the rules of the
  next section applied to the case's own program. It waits for a word or a
  flag, never for a time, and its computation ends when it is told to, not
  after so many turns: a runner that is slow or busy breaks any margin of
  time, and a computation sized by this machine runs for a minute on another.
  A bound, in a case or in a model, is counted in its own unit, looks, steps
  or turns until told, never in another thread's time.
- `TONDO_CASES_STALLS=<seed>` in the environment of the cases stops each
  case's program as a busy machine does: every thread at once, three times
  in its first second, and in the debug step one thread at a time inside the
  runtime, where the seed says. What counts on a margin of time, a case or a
  wait of the runtime, fails there and says when it stood still. A case red
  with a seed and green without holds a duration to find: the seed gives the
  same stops again, and a run without one only hides it.
- A window between two steps, where one thread changes in two what another
  reads as one, is found by a stop put inside it, not by rounds: a stall
  point (`stalls::here`) in the gap, which every seed of the stops opens,
  where a loop of hundreds of plain rounds may never meet it. The case that
  fails with the stop and passes with the fix is the test that stays.
- A model says at its top what it assumes of the code, and whoever changes
  that code reads the list: a model whose monitor looks at every step cannot
  see a monitor that sleeps.

### Waits in a test

A test that fails now and then on a slow machine has most often taken time
for a signal: it slept and went on, or gave a wait a cap that the machine
did not meet. Tests and the runtime hold to these rules:

- A test does not sleep to let something happen. It waits for what says
  that it happened: a line the program prints, an answer, a file, what a
  tool reports. The program under test says where it is from the place
  itself, and a tool that a script follows says when it is in place.
- A cap only bounds a hang. A run that passes never waits for it, so it is
  as long as the slowest machine needs, and a wait that includes a build
  has a build's margin.
- A sleep stays only where time is what is under test, as that nothing
  happens meanwhile, and a comment saying so starts with "Time is what is
  under test". What only asking can see, as a port that starts to listen,
  is waited for with `until` (`crates/tondo/tests/common`), whose pace is
  not a wait for something to happen.
- What depends on how events fall in time, as which changes make one
  batch, is tested with the events and the clock in the test's hands; the
  test over the real system keeps only what no stall can break.
- In the runtime, a timeout is the cap of a hang or a time the program
  asked for, never the way of waiting for something to happen; where
  nothing can wake a wait, as a lock tried before a fork, it asks at a
  pace, with a cap.

`cargo test -p tondo --test waits` holds `crates/tondo/tests` to it.
