# Tondo

`bin/tondo` is the whole toolchain: the compiler, its runtime, the standard
library and the test runner. Put `bin` on your `PATH`:

```sh
export PATH="$PWD/bin:$PATH"
tondo version
```

On macOS, the builds of `main` that CI makes are signed with a Developer
ID and notarized by Apple, so Gatekeeper runs a `tondo` downloaded with a
browser; it asks Apple the first time.

It links executables itself, with no C compiler and no development
packages: only the libraries the system has to run programs. `TONDO_CC`
names a C compiler to link with instead. Dependencies from git or archives
use `git`, `curl`, `tar` and `unzip`; `tondo bind` reads headers with a C
compiler, `cc` or the one `TONDO_CC` names, and uses `pkg-config` when it
is given a package.

```sh
tondo run hello.to          # compile and run a file
tondo build app --output app # a native executable from a project
tondo test app              # run a project's tests
tondo help                  # every command
```

`tondo setup` checks that it works and sets up Claude Code and Codex:
the Tondo skill, and `tondo mcp`, an MCP server whose tools do all of the
above, for every project. It installs the Tondo extension in VS Code,
Cursor, VSCodium and Windsurf too: the `.vsix` of `--vsix <file>`, or with
GitHub's `gh` the one of the latest good build.
