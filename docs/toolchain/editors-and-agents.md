# Editors and agents

The same `tondo` that compiles is the language server, the debug adapter
and the MCP server. An editor or a coding agent talks to that one binary,
so what it shows is what the compiler knows, at the version that is
installed, with nothing else to install or keep in step.

- [Setting it up](#setting-it-up)
- [The language server](#the-language-server)
- [VS Code and its family](#vs-code-and-its-family)
- [Other editors](#other-editors)
- [Coding agents](#coding-agents)
- [The MCP server](#the-mcp-server)
- [Runbooks](#runbooks)
- [Every command as data](#every-command-as-data)
- [The shell](#the-shell)

## Setting it up

```sh
tondo setup
```

`tondo setup` checks that `tondo` compiles and runs a program and that its
MCP server answers. Then it writes the Tondo skill for Claude Code and for
the agents of the Agent Skills standard, registers `tondo mcp` with Claude
Code and Codex, for those that are installed, and installs the extension in
VS Code, VS Code Insiders, Cursor, VSCodium and Windsurf, for those it
finds. Each line it prints says what was done, and when it stops, its last
line says what is missing and how to get it. Running it again updates
everything, and `tondo setup --json` says what is set up, as data.

## The language server

`tondo lsp` speaks the Language Server Protocol over standard input and
output.

| | |
| --- | --- |
| Diagnostics | as you type, for the whole workspace, each with its quick fixes and a link to what its code means |
| Completion | which writes the import of a module it names; in an `import`, the modules there are |
| Navigation | definition, type definition, implementations, references, call and type hierarchies, symbols |
| Reading | hover with documentation, signature help, inlay hints of types and parameters, semantic highlighting |
| Rename | of a name everywhere, and of files and folders with their imports |
| Refactors | change a signature, move a declaration to another module, inline a call, extract a trait, group parameters into a type, make a function a method and back |
| Formatting | of a file or a selection, with the imports a file lacks added and those it does not use removed |
| Tests | each test and suite with a lens to run or debug it, and how many tests reach each function |
| Files of one system | `name_linux.to` and `name_macos.to` are both checked, wherever the editor runs |

The refactors are checked by compiling: one that would leave the program
with an error is not offered as done.

## VS Code and its family

The extension, in [`editors/vscode`](../../editors/vscode), adds the rest
of the toolchain to the editor:

- **Run and debug** with F5 over `fn main`, a script or a test, through
  `tondo dap`: breakpoints with conditions, hit counts and log messages,
  the variables of every frame and task, a stop where a panic happens, and
  an array of records shown as a table.
- **Backwards.** In a replay, *Why Does It Have This Value?* on a variable
  goes to the line that gave it, *Where Does It Come From?* follows the
  chain back to what the program read, and a bar shows every value a
  binding took over the run.
- **Tests** in the Testing view, with coverage in the margin, *Test in
  Every Order* for a test that starts tasks, which offers to debug the
  order that failed, benchmarks against a baseline, and fuzz tests.
- **Live builds.** A program takes in what is saved and keeps running, and
  beside each line the editor shows the values it last produced.
- **A running program.** *Add a Logpoint to a Running Program*, *Show What
  a Running Program Waits For*, and profiles shown beside the code, each
  hot line with its share.
- **Dumps and recordings** open as debugging sessions. *Write a Test From
  a Dump* writes the test that fails as the dump did, and *Find the Commit
  That Broke a Test* bisects with it.
- **Projects.** *Tondo: New Project* makes one, and *Tondo: Get Started*
  walks through installing, running, debugging and testing.

It finds `tondo` on `PATH`, in `~/.cargo/bin` or in `~/.local/bin`, and the
setting `tondo.path` names another. An editor of the family that `tondo
setup` did not find takes the extension with `<editor> --install-extension
<file>`.

## Other editors

Any editor with an LSP client can use `tondo lsp` for `*.to` files. In
Helix, `languages.toml`:

```toml
[[language]]
name = "tondo"
scope = "source.tondo"
file-types = ["to"]
roots = ["tondo.toml"]
language-servers = ["tondo"]

[language-server.tondo]
command = "tondo"
args = ["lsp"]
```

In Neovim:

```lua
vim.filetype.add({ extension = { to = 'tondo' } })
vim.lsp.config('tondo', { cmd = { 'tondo', 'lsp' }, filetypes = { 'tondo' }, root_markers = { 'tondo.toml' } })
vim.lsp.enable('tondo')
```

`tondo dap` is the debugger for any editor that speaks the Debug Adapter
Protocol.

## Coding agents

Tondo was designed with models among its authors, and that shows in the
language before it shows in the tools: few ways to say a thing, signatures
that carry the whole contract, one layout, and errors that say what to
write. The tools then make each step of an agent's work a call that
returns data.

| An agent needs | Tondo gives |
| --- | --- |
| To learn a language it was not trained on | `tondo guide`, a topic at a time, with examples that compile; `tondo guide pitfalls`, the mistakes that habits from other languages cause |
| To know an API without reading its source | `tondo api <module>`, declarations and documentation without bodies; `tondo api --search` |
| To know whether what it wrote is right | `tondo check --json`: a code, a range and a message that says what to write |
| To repair it | `tondo fix`, and the `fixes` of each diagnostic as edits |
| To spend fewer tokens | `tondo fmt --compact`, the same code in about 15% fewer tokens; `tondo api`, a module without its bodies |
| To run a program safely | the `run` tool, with input, a timeout, and a panic as data |
| To find a bug | the debugger as tool calls; a recorded run to ask `why` of |
| To prove a change | `tondo test --affected`, `--schedules all`, `--bench --against` |

`tondo guide --skill` writes the skill that `tondo setup` installs: the
rules of the language and the commands, on a page a model reads at the
start of a session.

## The MCP server

`tondo mcp` serves the toolchain over the Model Context Protocol, on
standard input and output. `tondo setup` registers it; any other client
runs it so:

```json
{ "mcpServers": { "tondo": { "command": "tondo", "args": ["mcp"] } } }
```

Each tool's result comes as text for the model and as data, the JSON of
the matching command. Paths are relative to the server's directory, and
lines and columns count from 1.

| | Tools |
| --- | --- |
| Writing | `guide`, `api`, `api_diff`, `check`, `unreached`, `fix`, `refactor`, `format`, `compact`, `expand`, `explain`, `graph` |
| Navigating | `definition`, `references`, `implementations`, `calls`, `hover`, `complete`, `symbols`, `rename` |
| Projects | `new_project`, `add_dependency`, `remove_dependency`, `update`, `clean`, `bind`, `target_pack`, `setup`, `version` |
| Building and running | `build`, `run`, `test`, `test_bisect`, `doc_test`, `model` |
| Programs that keep running | `run_start`, `run_output`, `run_input`, `run_stop`, `reload`, `eval`, `values` |
| Debugging | `debug_start`, `debug`, `debug_stop`, `debug_compare`, `source` |
| A running program | `ps`, `counters`, `requests`, `blocked`, `logpoint`, `trace`, `profile`, `dump` |
| Failures | `dump_read`, `dump_test`, `dump_diff`, `dump_group`, `dump_trend`, `dump_issue`, `dump_report`, `dump_redact`, `dump_core` |
| Performance | `optimizations`, `asm`, `profile_show` |

Three things make it more than the commands under another name:

- **A program's input and a limit.** `run` takes `args` and `stdin` and
  ends a program that runs past `timeout`, so an agent cannot hang itself
  on a program that waits for a key.
- **Programs that keep running.** `run_start` gives a session for a
  server. After an edit, `reload` changes its code while it runs and keeps
  its state, where starting it again would lose the situation the agent
  was looking at. A panic in a session does not end the program: the task
  waits, and the next `reload` starts that function again.
- **A debugger with no terminal.** `debug_start` gives a session, on a
  program, a test in the order that failed, a recording or a dump, and
  `debug` sends it one command and returns the answers, the events and
  what the program printed.

## Runbooks

`tondo guide runbooks` is what to do when something goes wrong, step by
step, with the tools of `tondo mcp`: why each step, and what it gives.

| What you see | Topic |
| --- | --- |
| A program panics or crashes, or you have a dump of one | `runbook-crash` |
| A failure must not come back: a dump to a test that fails | `runbook-regression` |
| A program or a test does not end or does not answer | `runbook-hang` |
| You change a running program, without starting it again | `runbook-live` |
| A program does not compile | `runbook-compile` |
| A test fails sometimes, or tasks race | `runbook-flaky` |
| A value is wrong and nothing says where | `runbook-debug` |
| A request of a server is slow | `runbook-slow-request` |
| A program is slow, or a change may have made it so | `runbook-performance` |
| Memory keeps growing | `runbook-memory` |
| Failures come from many machines, or must be shared | `runbook-production` |
| You want to find the inputs that break a function | `runbook-fuzz` |
| You change code and must show it works | `runbook-testing` |

Every step of the runbooks runs in the tests of the toolchain, so what
they show is what the tools give. They are written for a model, and a
person in a hurry reads them the same way.

## Every command as data

With `--json`, a command writes one JSON object with its result, and
nothing else on standard output. Each has a `format` that names it and its
version, so a tool knows what it is reading:

```text
$ tondo version --json
{"backend":"cranelift","features":["json","graph","test-events","test-exact-many","test-watch","test-coverage","test-bench","test-fuzz","profile","optimizations","asm","lsp-tests","lsp-lenses","explain","new","add","fix"],"format":"tondo-version-0.1/1","ok":true,"target":"x86_64-linux","version":"0.1.0-dev"}
```

`features` says what this `tondo` can do, for a tool that must work with
more than one version. A diagnostic, in `tondo check --json`:

```json
{
  "code": "E1102",
  "file": "src/main.to",
  "id": "diag:905139dc15cc97049e75e8950b4a1d9f66ad03740c3d575800bf8af32a387eec",
  "message": "expected `String`, found `String?`, which may be `none`: give a fallback with `.unwrapOr(value)`, handle it with `match`, or pass it on with `?` in a function that returns an option",
  "module": "words",
  "range": {
    "start": { "byte": 365, "column": 27, "line": 15 },
    "end": { "byte": 369, "column": 31, "line": 15 }
  },
  "fixes": [],
  "related": [],
  "severity": "error"
}
```

`tondo test --test-format events` writes a line for each result as it
comes, for a tool that shows progress, and `tondo test --json` the whole
report at the end ([Testing](testing.md#reports-for-people-and-for-ci)).

## The shell

`tondo completions bash`, `zsh` or `fish` prints what the shell needs to
complete `tondo`'s commands and options. `tondo help` lists the commands by
what they are for, and [Every command](commands.md) has each with its
options.

Back to [The toolchain](../toolchain.md).
