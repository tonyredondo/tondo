# The command line

`tondo` is one binary: compiler, runner, test runner, formatter, language
server, MCP server, debugger and profiler. `tondo help` lists every
command, and `tondo help <command>` or `tondo <command> --help` explains
one. A path is a `.to` file or a project, a directory with `tondo.toml` or
`src/main.to`; without one, the current directory.

## Start

```text
tondo new shop                    a project: tondo.toml, src/main.to and a test
tondo init [<dir>] [--name <n>]   make a project of a directory that exists
tondo setup                       ready Claude Code, Codex and VS Code-like editors
tondo version                     this tondo's version, target and backend
tondo completions zsh             what bash, zsh or fish completes of tondo
```

## Write and check

```text
tondo check [<path>]              diagnostics only, each with its code and repair;
                                  of a project, of its tests too once its code is right
tondo fix [<path>]                apply the compiler's safe repairs, then check
tondo fmt <file.to>...            canonical layout, naming the files it changed; --check changes nothing
tondo fmt --compact <file.to>     the same code in about 15% fewer tokens
tondo explain E1102               what an error, warning or panic code means
tondo model apply                 write and run the test of a state machine, with no model to write
tondo expand [<path>]             what each `derive` wrote
tondo api std.http                a module's declarations, docs and examples
tondo api std.http.Client         one type, its methods and example: also String
tondo api --search base64         declarations by name and docs, std's included
tondo api --list                  every module: std, the project's, dependencies', internal aside
tondo guide [<topic>]             this guide; --list names the topics
tondo graph [<path>]              modules, files and imports; --dot or --mermaid
```

Every diagnostic says what to write. `tondo check --json` gives them as
data, with spans and, for many, `fixes` as edits; `safe` ones are what
`tondo fix` applies.

## Run and build

```text
tondo run app.to -- --port 8080   compile, cache and run, with the program's arguments
tondo app.to a b                  the same for a script; `#!/usr/bin/env tondo` works
tondo run --watch                 run again, or reload in place, as the files change
tondo run --live                  a live build: `tondo reload <pid|name>` changes its code
tondo build --output app          a native executable; --static, --strip, --debug
tondo build --target aarch64-linux
tondo build --library             a C library, lib<name>.so and .a, with its header
tondo target pack                 this machine's target, for a tondo elsewhere
tondo bind lib.h --library z      extern declarations for a C header
```

`run` keeps its build in the cache by the files it reads, so a second run of
the same code starts at once; `tondo clean` empties the cache.

## Test

```text
tondo test                        every test of the project
tondo test --filter parse         tests whose ids contain `parse`; --glob, --exact <id>
tondo api --diff v1.2              what the public API lost, changed or gained since a commit
tondo check --unreached            the code neither main nor a test reaches
tondo check --docs                 what is public and has no /// (W1010)
tondo check --performance          a copy a loop makes on every turn (W1009)
tondo optimizations                what a release build inlines, makes without the heap
                                   and checks, place by place; --kind heap, --function
tondo optimizations --against main what it makes differently from a git revision
tondo asm sum                      the machine code of a function, under its lines;
                                   --clif its IR, --against main what changed
tondo asm --all --compiler <tondo> the functions another tondo makes other code of;
                                   --rules, in a tondo built with them, Cranelift's rules
tondo test --affected main         the tests that reach what changed since a commit
tondo run --watch --tests          reloads on each change, and runs the tests it reaches
tondo eval app "total(order())"    what an expression gives in a running live build
tondo eval app "queue()" --every 1 and again each second, compiled once; --count ends it
tondo values app app.to            what each line left, with nothing stopped
tondo test --bisect v1.2 --filter parse   the first commit where a test fails; with --bench
                                  --max-regression 5, where a benchmark regressed
tondo test --watch                again, the tests a change affects, as files change
tondo test --coverage             with the lines, functions and branches that ran
tondo test --bench                the benchmarks (testing.bench), --against main
tondo test --fuzz --fuzz-time 30  search for inputs that fail a testing.fuzz test
tondo test --update-snapshots     record snapshots instead of checking them
tondo test --report junit=out.xml one or more reports: json, junit, lcov, markdown, html
tondo doc-test README.md          check the Tondo examples of a document
```

A warning you accept stays out of the list with a comment on its line or above
it, which says why:

```tondo
// tondo: allow W1004: the name C gives this type
pub alias CUShort = UInt16
```

Without a reason it is an error, and when its line stops giving the warning
`W1019` says to remove it. `tondo check --json` lists what it silenced in
`silenced`.

`tondo guide testing` says how to write tests, suites, snapshots, benchmarks
and fuzz tests.

## Dependencies

```text
tondo add json --git https://example.com/json.git --tag v1.2.0
tondo add util --path ../util     also --url <archive> --sha256 <hash>, --dev
tondo remove util
tondo update                      fetch again and rewrite tondo.lock
tondo clean --all                 the fetched dependencies too
```

## Observe a running program

A program started with `TONDO_DIAGNOSTICS=1`, or a debug build, can be
observed by its process ID or name:

```text
tondo ps                          the programs there are
tondo counters api --interval 1   the runtime's counters and the program's own
tondo trace api --duration 5      what its tasks do, for Perfetto, and a profile
tondo profile api --cpu           where it spends its time; --memory, --tasks
tondo profile -- ./app arg        a program it starts, until it ends
tondo profile show cpu.pb.gz      the functions and lines that take the most; --html;
                                  --asm, the hottest function's instructions with their share
tondo dump api                    its tasks, where each is, counters and live memory
tondo blocked api                 who waits for whom now
tondo logpoint api app.to:12 "order {order.id}"   what a line holds as it goes by
tondo logpoint api app.to:12 --if "total > 100" --dump d.json   the whole program, that time
tondo logpoint api app.to:12 --stop 60   a breakpoint of that task alone, in a Release build
tondo logpoint api app.to:12 --request /orders   only in the tasks of that request
tondo dump read api.dump why      what went wrong, as far as it can tell
tondo dump read api.dump stacks   the tasks by the stack they share
tondo dump read api.dump requests the HTTP and gRPC requests, each with its timeline
tondo requests api                what each request did, through every task it started
tondo dump read api.dump events   what it did last, with TONDO_DUMP on
tondo dump read api.dump find id  where a text is, in values, stacks and contexts
tondo dump read api.dump tasks    read a dump, as a crash leaves in ~/.cache/tondo/dumps
tondo dump read api.dump locals   the bindings of a frame; `print user.name` a part
tondo source lib/x.to --from a.dump  a file as the build had it, from its repository
tondo dump diff a.dump b.dump     what changed between two dumps of a program
tondo dump group dumps/           the dumps by the failure they are, the most frequent first
tondo dump group dumps/ --since v1.2   the failures new after a commit
tondo dump issue dumps/ 8517      a failure as an issue, in Markdown
tondo dump serve dumps/           the failures as a page here, up to date as dumps come
tondo dump report a.dump          all of it as one page of HTML; --output a file
tondo dump redact a.dump b.dump   a dump to share, without what the program held
tondo dump core core.123          a core of the system as a dump; --executable, --output
tondo dump core 4211              the core coredumpctl has of that process
tondo dump test a.dump            a test that fails as the dump did, with its values
tondo dump trend dumps/           what keeps growing over dumps: a leak, by its line
```

## Debug

```text
tondo debug app.to -- arg         break, step, locals, print, stack, tasks
tondo debug --test <id>           one test, as `tondo test --list` names it
tondo debug --live app.to         and `reload` the code where it stopped
tondo debug api                   a running Release build: breakpoints stop one task
tondo debug --via "ssh host" app  a program on another machine, where `tondo relay app` joins it;
                                  ps, counters, trace, profile, dump and reload take --via too
tondo dap                         the same for an editor, with the Debug Adapter Protocol
```

## Editors and agents

```text
tondo lsp                         the language server an editor starts
tondo mcp                         the MCP server an agent starts: see `tondo guide mcp`
```

## Output for tools

`--json` makes a command write one JSON object with `format`, as
`tondo-check-0.1/1`, `ok` and its result; `tondo test --test-format events`
gives a line per result as it runs. `--diagnostic-format json` gives the
diagnostics of `run`, `build` and `test` as JSON on standard error, and a
program run with `TONDO_PANIC_FORMAT=json` writes its failure as data. A
command exits with 0 when it did what was asked, 1 when its result is a
failure (diagnostics, a failing test), 2 for a usage error and 3 when
something stopped it.
