# Every command

`tondo` is one executable, and this is all of it: each command with its
options, as `tondo help --all` prints them. The chapters beside this page
show the commands at work; this one is for finding the exact option.

- `tondo help` lists the commands by what they are for, and
  `tondo help <group>` says what each command of a group does.
- `tondo <command> --help` gives one command's usage.
- `tondo guide cli` explains them in prose, for a person or a model.
- With `--json`, a command writes its result as one JSON object, which
  [Editors and agents](editors-and-agents.md) describes.

| Group | Commands |
| --- | --- |
| Write | `new`, `init`, `run`, `build`, `check`, `fix`, `fmt` |
| Testing | `test`, `doc-test`, `model` |
| Learn | `guide`, `api`, `explain`, `expand`, `graph` |
| Projects | `add`, `remove`, `update`, `clean`, `bind`, `target` |
| Observe | `ps`, `counters`, `requests`, `blocked`, `logpoint`, `trace`, `dump` |
| Debugging | `debug`, `dap`, `relay`, `source`, `reload`, `values`, `eval` |
| Performance | `profile`, `optimizations`, `asm`, `compiler` |
| Tools | `lsp`, `mcp`, `setup`, `completions`, `help`, `version` |

A project is a directory with `src/main.to`, or with `main.to` or
`tondo.toml`. Where a command takes `<project>`, the current directory is
the default, and `--project <dir>` names another.

<!-- From here on, this page is what `tondo help --all` prints: `TONDO_UPDATE_DOCS=1 cargo test -p tondo --test docs` writes it again. -->

```text
New to Tondo, or a model writing it? Start with `tondo guide`.

usage:
  tondo run [<project>|<file.to>] [--link <library>]... [--static] [--debug | --live | --watch [--tests]]
            [--target <target>] [--cpu <level>] [--diagnostic-format human|json] [-- <argument>...]
  tondo run --record <recording> [--last <duration> [--hold <duration>]] [<project>|<file.to>]
            [--link <library>]...
            [-- <argument>...]                         compile and run a program (under qemu for another
                                                       machine; live: its code changes as it runs, with
                                                       `tondo reload`, or by itself as its files change;
                                                       recorded, for `tondo debug --replay`)
  tondo build [<project>|<file.to>] [--output <path>] [--link <library>]... [--static] [--strip] [--no-source-links]
            [--debug | --live] [--target <target>] [--cpu <level>] [--diagnostic-format human|json] [--json]
                                                       compile a native executable (static on Linux;
                                                       without its symbol table; static for a target:
                                                       x86_64-linux or aarch64-linux; for a level of
                                                       processor: baseline, x86-64-v3, x86-64-v4, native)
  tondo build --library [<project>|<file.to>] [--output <dir>] [--debug] [--cpu <level>] [--json]
                                                       compile a library for C, lib<name>.so (.dylib)
                                                       and lib<name>.a, with its header <name>.h
  tondo check [<project>|<file.to>] [--library] [--unreached] [--docs] [--performance] [--json]
                                                       report diagnostics only (of a library; as JSON);
                                                       --unreached, the code neither main nor a test reaches
  tondo fix [<project>|<file.to>] [--library] [--json]
                                                       apply the compiler's safe repairs to the program's files
  tondo expand [<project>|<file.to>] [--json]          show what each `derive` expanded to
  tondo fmt [--check | --compact] [--json] <file.to>...
                                                       format files canonically, or print them compactly
  tondo api [<module>|<module>.<declaration>|<file.to>] [--private] [--list] [--project <dir>] [--json]
  tondo api --search <words> [--project <dir>] [--json]
  tondo api --diff <commit> [--project <dir>] [--json]
                                                       show a module's declarations, documentation and examples,
                                                       or one type with its methods and example (std.http.Client,
                                                       String); find declarations by their names and documentation
  tondo graph [<project>|<dir>|<file.to>] [--packages | --files] [--module <module> [--reverse]]
            [--json | --dot | --mermaid]               show what a program is made of: its packages,
                                                       modules and files, and what each imports
  tondo guide [<topic>] [--list] [--skill] [--json]    learn Tondo: the language, the library, the commands
  tondo model [<project>] <function> [--json]          write and run the test of a state machine of the program, with no model to write
  tondo explain [<code>] [--json]                      what the code of an error, a warning or a panic means, with examples
  tondo update [<project>] [<dependency>...] [--json]  fetch dependencies again and rewrite tondo.lock
  tondo new <name> [--json]                            make a project in a new directory: tondo.toml,
                                                       src/main.to and a test
  tondo init [<dir>] [--name <name>] [--json]          make a project of an existing directory
  tondo add <alias> --path <dir> | --git <url> [--tag <tag> | --branch <branch> | --rev <commit>]
            | --url <url> [--sha256 <hash>] [--dev] [--project <dir>] [--json]
                                                       add a dependency to tondo.toml, and fetch it
  tondo remove <alias> [--project <dir>] [--json]      take a dependency out of tondo.toml
  tondo clean [--all] [--json]                         empty the cache of builds; with --all, the fetched
                                                       dependencies too
  tondo test [<project>] [--filter <text> | --glob <pattern> | --exact <id>... | --affected <rev>]
            [--bisect <commit> [--bisect-with <command>]] [--shard <i/n>] [--watch]
            [--order canonical|random] [--schedules <n>|all|all:<n>|at:<choices>] [--seed <hex>] [--record <dir>] [--list] [--jobs <n>]
            [--timeout <seconds>]
            [--retry <n>] [--allow-flaky] [--lock-order] [--repeat <n>] [--codeowners auto|none|<path>]
            [--show-output] [--allow-empty] [--deny-skips] [--test-format human|json|events] [--json]
            [--report json=<path> | junit=<path> | coverage=<path> | lcov=<path> | markdown=<path>
                      | html=<path>]... [--coverage]
            [--bench [--bench-time <seconds>] [--save-baseline <name>] [--baseline <name> | --against <rev> | --compiler <tondo|rev>]
                     [--max-regression <percent> [--regression-by time|instructions]] [--layouts <n>]]
            [--profile cpu|memory]
            [--fuzz [--fuzz-time <seconds>] [--fuzz-timeout <seconds>] [--fuzz-memory <MiB>] [--fuzz-corpus <dir>]]
            [--models]
            [--diagnostic-format human|json] [--artifacts <dir>] [--update-snapshots]
            [--static] [--debug] [--library] [--target <target>] [--cpu <level>] [--exec <command>]
                                                       run the tests of a project (of a library); with --watch,
                                                       again as its files change; --bench measures its benchmarks,
                                                       --fuzz searches for inputs that fail its fuzz tests, and
                                                       --models checks the models of `std.model` alone; --exec runs
                                                       the tests' executable under a command, as `perf record -g`
  tondo doc-test [--edition 0.1] <markdown>            check the examples of a document (as JSON)
  tondo lsp                                            serve an editor: diagnostics, hover, definitions,
                                                       references, rename, completion, highlighting
  tondo mcp                                            serve an agent, with the Model Context Protocol:
                                                       every command above as its tools
  tondo setup [--vsix <file>] [--json]                 set up Claude Code and Codex: the skill, and tondo
                                                       mcp for every project; and the extension of VS Code
                                                       and its kin; checked
  tondo ps [--via <command>] [--json]                  list the running Tondo programs
  tondo source <file> --from <dump|recording|executable> [--line <n>] [--json]
                                                       the file of a build as it was: here if it is the same,
                                                       or from its repository, at the build's commit
  tondo logpoint <pid|name> <file:line> [<text>] [--if <condition>] [--request <text>] [--dump <file>] [--stop <seconds>] [--max <n>] [--for <seconds>] [--via <command>] [--json]
                                                       what a line of a running program holds as it goes
                                                       by, without stopping it, in any build
  tondo blocked <pid|name> [--via <command>] [--json]  say who waits for whom in a running program
  tondo requests <pid|name> [--via <command>] [--json]
                                                       say what each request of a running program did, task by task
  tondo counters <pid|name> [--interval <seconds>] [--count <n>] [--via <command>] [--json]
                                                       watch a running program's counters, the runtime's
                                                       and its own (std.diagnostics)
  tondo debug [--json] [--live] <file.to|project|executable|pid|name> [-- <argument>...]
  tondo debug [--json] --test <id> [--schedule <seed>] [<project>]
  tondo debug [--json] --replay <recording>
  tondo debug [--json] --compare <recording> <recording>
  tondo debug [--json] --via <command> <pid|name>      debug a program built with --debug: breakpoints,
                                                       steps, bindings, stacks, tasks; and with --live,
                                                       `reload` to change its code where it stopped
  tondo relay <pid|name>                               join standard input and output to a running
                                                       program, for `debug --via "ssh host"`
  tondo values <pid|name> <file> [--for <seconds>] [--json]
                                                       what each line of a file left in a running live build,
                                                       with nothing stopped
  tondo eval <pid|name> <expression> [--every <seconds> [--count <n>]] [--json]
                                                       what an expression gives in a running live build,
                                                       with its functions and types as they are now
  tondo reload <pid|name> [--via <command>] [--json]   change a running live build's code to what its
                                                       files say now
  tondo dap [--link <library>]... [--static]           debug for an editor, with the Debug Adapter Protocol
  tondo dump <pid|name> [--output <path>] [--via <command>] [--json]
                                                       write a dump of a running program: its tasks and
                                                       where they are, its counters, its live memory
  tondo dump read <file> [why | tasks | stacks | requests | events | sources | task <id>
                         | locals [<task> [<frame>]]
                         | print <path> [<task> [<frame>]] | find <text>
                         | heap | objects | object <address>] [--json]
                                                       show a dump, as a crash leaves in ~/.cache/tondo/dumps
                                                       or TONDO_DUMP=<dir>, or the last in a log, as TONDO_DUMP=stderr
                                                       writes it
  tondo dump diff <file> <file> [--json]               what changed from one dump of a program to a later
                                                       one: memory by place, tasks, bindings, counters
  tondo dump group <dir> [--since <commit>] [--json]   the dumps of a directory by the failure they are,
                                                       the most frequent first; those new after a commit
  tondo dump serve <dir> [--port <n> | --output <file>]
                                                       the failures of a directory as a page on this machine,
                                                       up to date as dumps come; --output writes it once
  tondo dump issue <dir> <signature>                   a failure of a directory of dumps as an issue to
                                                       file, in Markdown
  tondo dump test <file> [--frame <n>] [--project <dir>] [--json]
                                                       a test that calls the function that failed with
                                                       what the dump says it had
  tondo dump core <core|pid|-> [--executable <file>] [--output <path>] [--json]
                                                       a core of the system as a dump: the signal, and
                                                       where each task and thread was; by pid from
                                                       coredumpctl, - from a pipe, as core_pattern gives it
  tondo dump redact <file> <output>                    the dump without what the program held, to attach
                                                       where anyone reads it
  tondo dump report <file> [--output <path>]           all a dump says, as one page of HTML
  tondo dump trend <dir> [--json]                      what keeps growing over the dumps of a program in a
                                                       directory, as TONDO_DUMP_WHEN leaves them
  tondo dump plugin <lldb|gdb> [--json]                write the `tondo` command for lldb or gdb, which
                                                       shows a stopped program as `dump read` does, and
                                                       a frame's bindings with its `locals` and `print`
  tondo trace <pid|name> [--duration <seconds>] [--output <path>] [--via <command>] [--json]
  tondo trace [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]
                                                       record what a program's tasks do, for Perfetto,
                                                       and a profile of where it spends its time
  tondo profile <pid|name> [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--via <command>] [--json]
  tondo profile [--cpu | --memory | --tasks] [--duration <seconds>] [--output <path>] [--json] -- <program> [<argument>...]
  tondo profile show <profile> [--top <n>] [--lines | --asm [<function>]] [--sample <type>] [--html <path>] [--json]
                                                       where a program spends its time, what it allocates and
                                                       holds, or where its tasks are, in pprof's format; show
                                                       reads one: the functions and lines that take the most
  tondo optimizations [<project>|<file.to>] [--tests] [--function <name>] [--kind <kind>]
            [--cpu <level>] [--against <rev> | --compiler <tondo|rev>] [--json]
                                                       what a release build makes of the code: the calls it
                                                       inlines and those it keeps, the texts and closures made
                                                       without the heap, the checks it drops and those it keeps
  tondo asm [<project>|<file.to>] [--tests] <function>|--all [--clif] [--rules] [--against <rev> | --compiler <tondo|rev>]
            [--cpu <level>] [--json]
  tondo asm --runtime <function> [--compiler <tondo|rev>] [--json]
                                                       the machine code a release build makes of a function, by
                                                       the lines of the program, with what the passes did there;
                                                       its IR before and after Cranelift optimizes it
  tondo compiler <tondo|rev> [--json]                  where the tondo of a revision of tondo's repository is,
                                                       built once in release, as --compiler takes it
  tondo target pack [<dir>] [--json]                   write this machine's target, for a tondo elsewhere
                                                       to build for it (TONDO_TARGETS=<dir>)
  tondo bind [<header.h>...] [--package <name>] [--library <library>]... [--project <dir>] [--output <file.to>] [--included] [--fails <file>]
            [-I <dir>] [-D <name[=value]>] [--json]    write a module of extern declarations for a C header
  tondo completions bash|zsh|fish                      what a shell completes of tondo's commands
  tondo help [<group> | <command> | --all]             the groups of commands; what a group's commands do;
                                                       a command's usage; every usage
  tondo version [--json]                               this tondo's version, target and backend
```
