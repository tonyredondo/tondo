# Projects

A single file runs as it is: `tondo run tool.to`. A project is a directory:

```text
app/
  tondo.toml          [package] name = "app"
  src/main.to         module app: fn main()
  src/models/user.to  module app.models (every file of a directory is one module)
  src/api/user.proto  module app.api.user (a `.proto` file is a module of its own)
  src/models/role.to  module app.models
  tests/api.to        an integration test root
```

```text
// src/models/user.to
pub type User = { name: String }

pub fn greet(user: User): String {
    "hello {user.name}"
}

// src/main.to
import std.console
import app.models                        // its names as models.User; or
                                         // `import app.models as m`, as m.User

fn main() {
    console.println(models.greet(models.User { name: "Ada" }))
}
```

- A module is a directory; `pub` makes a declaration visible to other
  modules. Imports name the package first: `app.models`.
- A module under a directory `internal`, as `app.internal.pricing`, is its
  package's alone: other packages cannot import it, and it can change without
  breaking them. No annotation says so: where it lives does.
- `import std.x` brings a standard module; there is nothing to install.
- Modules cannot import each other in a cycle.
- Code for one system or one architecture goes in its own files, which
  their names mark: `watch_linux.to`, `watch_macos.to`, and
  `signals_unix.to` for both; `crc_x64.to` and `crc_arm64.to`; and a system
  then an architecture, `poll_linux_arm64.to`, also before `_test.to`. A
  build takes those of its target; common code calls what each of them
  declares with the same signature, and `tondo check` checks every target,
  so an error of macOS shows on Linux as `on macOS:`, and one of arm64 as
  `on arm64:` or `on Linux arm64:`. There is no `#if`: `env.build().target`
  tells a program where it runs. `systems = ["linux"]` and
  `architectures = ["x64"]` in `[package]` limit a package.

## tondo.toml

```text
[package]
name = "app"

[dependencies]
units = { path = "libs/units" }
http = { git = "https://github.com/acme/http", tag = "v1.2.0" }
zlib = { url = "https://example.com/zlib-1.0.tar.gz" }

[native]
libraries = ["sqlite3"]               # C libraries for `extern fn`

[source]                              # a line's page, on a host of your own
url = "https://git.acme.com/{repo}/blob/{commit}/{path}#L{line}"
```

A dependency is imported by its alias: `import units`, `import units.si`.
A package of declarations only, with no `main` and no statements, is a
library: `tondo check` and `tondo test` work on it, and only `run` and
`build` need something to run.
`tondo new app` makes a project; `tondo add units --path libs/units` (or
`--git <url> --tag v1.2.0`, `--url <url>`) adds a dependency and fetches it,
and `tondo remove units` takes it out.
`tondo.lock` pins every download; `tondo update` refreshes it. `tondo api
--list` shows every module of the project, its dependencies and the
standard library.

## Executables

`tondo build app --output app` writes a native executable, which `tondo`
links itself: no C compiler is needed. On Linux, `--static` makes it
static, with musl, for a container with nothing else in it; its native
libraries must then be static too, `"static:z"` or a `.a` file.
`--strip` leaves out the symbol table. `tondo build --library` writes
libraries for C instead, shared and static, with their header (topic
`ffi`).

`--target x86_64-linux` or `--target aarch64-linux` builds a static
executable for that machine from any system. Another architecture's
runtime comes in a pack that `tondo target pack <dir>` writes on a machine
of it; `TONDO_TARGETS=<dir>` finds it. `tondo run` and `tondo test` run
such a program under qemu-user.

A build is for a level of processor: `baseline` unless `--cpu` asks for
another, or `cpu` in the `[build]` of `tondo.toml`. On x64, `baseline` is
x86-64-v2, up to SSE4.2 and POPCNT; `x86-64-v3` adds AVX2, BMI2 and FMA,
`x86-64-v4` some of AVX-512, and `native` takes what the machine that
builds has. A program checks at its start that its processor
has what its level needs, and says which level to build it for when not.
`env.build().cpu` names its level. Beyond it, `cpu.has(cpu.Feature.Avx2)`
asks the processor; a feature the level has, or one of the other
architecture, is known when compiling, and only the way of the `if` that
runs is compiled.

`std.cpu.x64` and `std.cpu.arm64` have functions that are one instruction
each, as `x64.depositBits` (BMI2's `pdep`), for the files of their
architecture. A function that uses one needs its feature, and is compiled
for it; call such a function under `if cpu.has(cpu.Feature.Bmi2) { ... }`,
with the plain way in the `else`. `main`, a test, a `pub extern fn`, a
trait's method and a function used as a value cannot need a feature: the
error says which call brings it in.

## Editors

In VS Code, and Cursor, VSCodium and Windsurf, the Tondo extension
(`editors/vscode`), which `tondo setup` installs, brings all of it: the
language server below, debugging with `tondo dap`, live builds that change
as files are saved, the tests in the Testing view, profiles with each hot
line's share at its end, tasks, and the tools of `tondo mcp` for VS Code's
agents. *Tondo: Get Started* walks through it.

`tondo lsp` is a language server: diagnostics as you type, hover, go to
definition, references, rename, completion, signatures, inlay types,
semantic highlighting, symbols, formatting, and each error's repairs as
quick fixes. Point any editor's
LSP client at it for `*.to` files; in Helix, `languages.toml`:

```text
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

In Neovim: `vim.lsp.config('tondo', { cmd = { 'tondo', 'lsp' }, filetypes = { 'tondo' }, root_markers = { 'tondo.toml' } })`
and `vim.lsp.enable('tondo')`, with `vim.filetype.add({ extension = { to = 'tondo' } })`.

## Agents

`tondo setup` checks that `tondo` works and sets up Claude Code and Codex,
those installed: it writes this guide's skill where they read skills and
registers `tondo mcp` for every project. Running it again updates them.

`tondo mcp` is a Model Context Protocol server: every command as a tool
with structured results, the language server's navigation and rename, and
debugging sessions. By hand, add it to Claude Code with
`claude mcp add --scope user tondo -- tondo mcp`, to Codex with
`codex mcp add tondo -- tondo mcp`, or to any client that reads this:

```text
{ "mcpServers": { "tondo": { "command": "tondo", "args": ["mcp"] } } }
```

`tondo guide --skill` prints the overview of this guide as an agent
skill: save it as `~/.claude/skills/tondo/SKILL.md` for Claude Code, or as
`~/.agents/skills/tondo/SKILL.md` for Codex and the other agents of the
Agent Skills standard.

`check` reports each diagnostic with its fixes, and `fix` applies the safe
ones; `run` takes the program's input and ends it after a timeout; `test`
gives the report of each test; `debug_start` returns a session for `debug`,
which takes the debugger's commands. `run_start` keeps a program running
as a live build: `run_input` and `run_output` talk to it, and `reload`
changes its code after an edit, without starting it again. Lines and columns count from 1, and
paths are relative to the directory the server runs in.
