# Tondo for VS Code

Everything `tondo` offers an editor, in VS Code: the language server, the
debugger, the tests, live builds, and the tools of `tondo mcp` for agents.

It needs the `tondo` executable: `sh dist/install.sh` from Tondo's
repository installs it. The extension finds it on `PATH`, in
`~/.cargo/bin` or in `~/.local/bin`; the setting `tondo.path` names another.
It says so when that `tondo` is older than the extension.

## What it does

- **Editing**, from `tondo lsp`: diagnostics as you type, with their quick
  fixes; hover; go to definition; references; rename; completion;
  signature help; inlay hints; symbols; semantic highlighting; *Run* and
  *Debug* over `main` or a script; and formatting, on save by default.
- **Running and debugging**, from `tondo dap`: *Run* and *Debug* over
  `fn main`, in the editor's title bar, or with F5 on any Tondo file, which
  runs its project or the file alone as a script. Breakpoints, stepping,
  bindings, conditions, and *Restart Frame*.
- **Live builds**: *Tondo: Debug Live* starts a build whose code changes
  while it runs: saving a file puts its new code in, keeping the program's
  state (Edit and Continue). *Tondo: Run Live* does the same in a terminal.
- **Tests** in the Testing view: every test and suite of the workspace's
  projects, with buttons beside them in the editor, each result and what it
  writes shown as they come. A failure shows its message on the line that
  failed, a comparison as a diff, and its stack; *Debug Test* runs one test
  alone under the debugger. Tests appear as you write them, before the
  file is saved, a new project's too; *Run Tests Continuously* runs again
  those that a saved change can affect, and *Run with Coverage* shows which
  lines, functions and ways of branches the tests ran.
- **Tasks**: check, build, run, test and run live, with their errors in
  the Problems panel.
- **Agents**: VS Code's agents get the tools of `tondo mcp` (check, fix,
  run, test, debug, reload and the rest) without any setup.

## Settings

- `tondo.path`: the `tondo` executable, when it is not found by itself.
- `tondo.reloadOnSave`: whether saving a file reloads a live build being
  debugged (on by default).
