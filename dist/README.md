# Tondo

`bin/tondo` is the whole toolchain: the compiler, its runtime, the standard
library and the test runner. Put `bin` on your `PATH`:

```sh
export PATH="$PWD/bin:$PATH"
tondo version
```

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

For a coding agent, the skill and the MCP server, whose tools do all of
the above:

```sh
mkdir -p ~/.claude/skills/tondo && tondo guide --skill > ~/.claude/skills/tondo/SKILL.md
mkdir -p ~/.agents/skills/tondo && tondo guide --skill > ~/.agents/skills/tondo/SKILL.md
claude mcp add --scope user tondo -- tondo mcp     # Claude Code
codex mcp add tondo -- tondo mcp                   # Codex
```
