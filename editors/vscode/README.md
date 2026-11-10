# Tondo for VS Code

Everything `tondo` offers an editor, in VS Code: the language server, the
debugger, the tests, live builds, and the tools of `tondo mcp` for agents.

It needs the `tondo` executable: `sh dist/install.sh` from Tondo's
repository installs it, and this extension in the editors it finds. The
extension finds `tondo` on `PATH`, in `~/.cargo/bin` or in `~/.local/bin`;
the setting `tondo.path` names another. The status bar shows its version
and the system programs run and are debugged for, or says that it is
missing or older than the extension, and opens the Tondo menu. *Tondo: Get Started* walks through installing it, making a
project, running, debugging and testing it.

## What it does

- **Projects**: *Tondo: New Project* makes one, with a program and a test,
  and opens it.
- **Editing**, from `tondo lsp`: diagnostics as you type, those of the
  whole workspace too, with their quick fixes and a link to what their
  code means; hover; go to definition, type definition and
  implementations; references; call and type hierarchies; how many tests reach each function; group parameters
  into a type, extract a trait, make a function a method and back, inline an
  alias, delete what nothing uses; change a signature,
  move a declaration to another module and inline a call, checked by compiling;
  rename, of
  files and folders too; completion, which writes the import of a module
  it names, and in an `import` lists the modules there are, each standard
  one with what it offers; signature help; inlay hints of types and parameters; symbols;
  folding and smart selection; semantic highlighting; *Run* and *Debug*
  over `main` or a script; and formatting, of a selection too, on save by
  default, when the imports a file lacks are added and those it does not use
  removed.
- **Files of each system**: `watch_linux.to`, `watch_macos.to` and
  `signals_unix.to` carry a badge with their system, never greyed out,
  since tondo checks every system: an error of the other one shows as
  `on macOS: ...`, its files are read as that system reads them, and go to
  definition offers each system's declaration. Their breakpoints stay
  unverified with the reason, and their tests show without a run button.
- **Running and debugging**, from `tondo dap`: *Run* and *Debug* over
  `fn main`, in the editor's title bar, or with F5 on any Tondo file, which
  runs its project or the file alone as a script. Breakpoints with
  conditions, hit counts and log messages; stepping; the variables of every
  frame and task, opened field by field; watches and hovers of paths like
  `user.name`; changing a value; a stop where a panic happens, before it
  unwinds; *Restart Frame*; *Show as Table* on an array or a map of
  records, a table that sorts by its columns and filters its rows;
  *Why Does It Have This Value?* on a variable or the code, which goes back
  to the line that gave the value, in a replay or with `"record": true`,
  and offers to ask the same of each value that line read; *Where Does It
  Come From?*, in a replay, the whole chain back to what the program read;
  *Go to When a Condition Holds*, the first or the last time in the run;
  *Compare with Another Moment*, what changed since another probe;
  a frame in a file that is not here as it was built opens it from its
  repository, at the build's commit, as if it were here, with breakpoints;
  *Open Dump Report* shows all a dump says beside the code, with places that
  open, and while it is open a hover says what its frames held;
  *Add a Logpoint to a Running Program* shows what the line of the cursor holds
  in a running program, in a panel, without stopping it; *Stop a Task of a Running Program
  Here* a breakpoint of that task alone, in a build without probes; *Show Failures From
  Dumps* marks in the code where the dumps of a directory fell, and the locks
  they found taken in orders that can deadlock; *Debug a Dump* opens a core of the
  system too, and *Read a Core of the System* one whose program is elsewhere;
  *Write a Test From a Dump* a test that fails as the dump did, *Find the Commit That
  Broke a Test* when it came, *Write a Failure as an Issue* and *Redact a Dump* what to
  share; *Run the Tests a Change Reaches*; *Run the Tests Looking at the Order of
  Locks*; *Show What a Running Program Waits For*;
  *Compare Two Dumps* and *What Keeps Growing Over Dumps*, what changed between
  dumps of a program and what grows over them;
  *Compare Two Recordings*, where a run that passed and one that failed part
  and why;
  *Open Disassembly View* and *View Binary Data* on a binding, whose bytes a
  live program lets you change; and a long search of a replay shows its
  progress and can be cancelled;
  *Show Its Values
  over the Run*, every value a binding took, from which line, on a bar of a
  replay whose marks take the program there; beside each line run, with
  `"record"`, what it changed the last time; *Show What the Tasks Wait For*
  in the call stack's menu, the tasks as a graph that points at who holds
  what each waits for; with `"followChildren": true`, each program it
  starts in a session of its own, which a debug build of Tondo waits for;
  and *Debug a Dump*, which opens a dump of a crashed program at its tasks
  and stacks.
- **Live builds**: *Tondo: Debug Live* starts a build whose code changes
  while it runs: saving a file puts its new code in, keeping the program's
  state (Edit and Continue). *Tondo: Run Live* does the same in a terminal.
  While one runs, the status bar says *Live*, and after each change what
  became of it: applied; applied, but with frames that finish on the old
  code; not applied because the files do not compile, while the program
  goes on with its old code; or needing the program started again, which
  Run Live does by itself and Debug Live offers.
- **Tests** in the Testing view: every test and suite of the workspace's
  projects, with buttons beside them in the editor, each result and what it
  writes shown as they come. A failure shows its message on the line that
  failed, a comparison as a diff, and its stack; *Debug Test* runs one test
  alone under the debugger. Tests appear as you write them, before the
  file is saved, a new project's too; *Run Tests Continuously* runs again
  those that a saved change can affect, and *Run with Coverage* shows which
  lines, functions and ways of branches the tests ran. A snapshot that
  differs offers *Update Snapshot*. The examples of the workspace's
  Markdown documents are there too, checked by `tondo doc-test`. The
  *Benchmark* profile measures the tests that call `testing.bench`: each
  benchmark's time, spread and allocations go to its output, and the
  report opens beside the code, compared with a baseline when the settings
  give one. *Why It Fails Sometimes*, on a test, runs it until it passes and
  fails, and says where the two runs part. A test that calls `testing.fuzz` is searched with *Fuzz Test*,
  beside it in the Testing view and in the menu of its button in the
  editor, until its notification's Stop or `tondo.fuzz.time` seconds: how
  the search goes shows in the test's output, and an input that fails, as
  small as it still fails, in the failure, with a link to its file in the
  project's `fuzz/` store, which every run tries from then on. Each kind
  of test is marked, beside its name and at the end of its line in the
  editor: `⏱ benchmark`, `🔎 fuzz`. What a test says of itself with
  `testing.tags` shows beside its name too after it runs, as
  `component=users, kind=integration`, and in its output.
- **Modules**: the *Tondo Modules* view shows each project's packages,
  modules, files and imports, from `tondo graph`; *Show the Module Graph*
  writes it in Mermaid.
- **Profiles**: *Profile* over `main` runs the program under `tondo profile`
  in a terminal, until it ends or is stopped; *Profile Test*, the flame
  beside each test in the Testing view and in the menu of its button in
  the editor, profiles the tests chosen, or what benchmarks measure. A
  test needs a millisecond of processor for a sample: fast code is
  profiled through a benchmark. Each
  hot line of the program then shows its share at its end, as `🔥 64.4%
  (94ms)`, and the flame graph opens beside the code. *Show a Profile…*
  opens any pprof file, a CI artifact or one `std.profile` wrote, and
  *Clear the Profile* takes it away.
- **Optimizations**: *Show the Optimizations* tells, from `tondo
  optimizations`, what a release build makes of the program of the file
  open: the end of each line says what costs there, `⚙ allocates`, `keeps a
  check` in a loop or `a call stays`, and over the line is everything the
  compiler did there, what it inlined, made without the heap or stopped
  checking, and why it did not. Saving tells it again; *Clear the
  Optimizations* takes it away.
- **Running programs**, those started with `TONDO_DIAGNOSTICS=1` and debug
  builds: *Attach* picks them by name; *Watch a Running Program's
  Counters* shows them in the *Tondo Counters* view as they change; *Trace
  a Running Program* records it and opens the timeline in Perfetto; *Dump
  a Running Program* shows where its tasks are and what holds its memory.
- **Tasks**: check, build, run, test and run live. Check and build read
  the diagnostics of `tondo` as data into the Problems panel, with their
  ranges.
- **Agents**: VS Code's agents get the tools of `tondo mcp` (check, fix,
  run, test, debug, reload and the rest) without any setup.

## Settings

- `tondo.path`: the `tondo` executable, when it is not found by itself, or
  the directory it is in, as a download's folder; `~` is your home.
  Changing it starts the language server again.
- `tondo.reloadOnSave`: whether a live build takes in its files when they
  are saved or change (on by default); off, *Tondo: Reload* does it.
- `tondo.run.args` and `tondo.run.env`: the arguments and the variables of
  the programs run or debugged from the editor without a `launch.json`,
  and of the tasks; a `launch.json` gives its own `args` and `env`, and with
  `last`, as `"5m"`, it runs recorded in a terminal and a run that fails is
  replayed from the end it kept (`hold`, `recording`).
- `tondo.test.args` and `tondo.test.env`: what `tondo test` takes in the
  Testing view, as `["--jobs", "4"]`, and the tests' variables.
- `tondo.bench.args`: what the *Benchmark* profile adds to `tondo test
  --bench`, as `["--against", "main", "--max-regression", "5"]`.
- `tondo.fuzz.time`: how many seconds *Fuzz Test* searches for; `0`, the
  default, until it is stopped.
