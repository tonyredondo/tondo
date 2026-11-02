# The MCP server

`tondo mcp` gives an agent the whole toolchain as tools of the Model Context
Protocol, over standard input and output. `tondo setup` registers it with
Claude Code and Codex for every project; any other client runs it so:

```text
{ "mcpServers": { "tondo": { "command": "tondo", "args": ["mcp"] } } }
```

Paths are relative to the server's directory, lines and columns count from
1, columns in characters, and each result comes as text for the model and
as data (`structuredContent`), the JSON of the matching command. A program
that panics or fails in `main` gives `panic`: its code, message, data,
source and stack, as data.

## Writing code

1. `guide`, the overview, then the topics it names; `pitfalls` before
   writing much.
2. `api` before calling what you have not used: `search` finds what does
   a job, then `api` with a declaration (`std.http.Client`, `String`,
   `Option`) gives a type, its methods and an example that compiles, which
   every type of the standard library has; with a `module` (`std.http`,
   `prelude`, a module of the project), all it declares; `list` names the
   modules. `hover` and `complete` work at a place of a file.
3. Write, then `check`: each diagnostic has an id and says what to write.
   `fix` applies the safe repairs, or those chosen by id.
4. `format` the files written; `compact` shows one in fewer tokens.
5. `run` or `test`; `explain` says what a code of an error or a panic means.

Tools: `check`, `fix`, `format`, `compact`, `expand`, `api`, `guide`,
`graph` (modules, files and imports), `explain`.

## Navigating

`definition`, `references`, `implementations`, `calls` (incoming, or with
`direction: outgoing`), `hover`, `complete` and `symbols` (a file's, or
with `query` the project's) take a `file`, `line` and `column`; `rename`
also a `new_name`, and rewrites the files.

## Projects

`new_project` makes one (`init` for a directory that exists),
`add_dependency` adds one from a `path`, a `git` repository with `tag`,
`branch` or `rev`, or a `url` with its `sha256`, `remove_dependency` takes
one out, and `update` fetches them again.

## Building, running and testing

- `build`: an executable, or with `library` a library for C; `static`,
  `debug`, `target`, `link`.
- `run`: builds and runs with `args` and `stdin`, and ends a program that
  runs past `timeout`, 60 seconds by default.
- `test`: `filter`, `glob` or `exact` choose tests and `list` lists them;
  `coverage`; `update_snapshots`; `bench` with `baseline`, `save_baseline`
  or `against` a git revision, and `max_regression`; `fuzz` with a number
  of seconds; `flags` takes any other option of `tondo test`.
- `doc_test` checks a Markdown document's examples, `bind` writes extern
  declarations for C headers, and `target_pack` this machine's target.

## Programs that keep running

`run_start` starts one, a server or anything interactive, and gives a
session; `watch` reloads it as its files change. `run_output` gives what it
printed since the last time, `run_input` writes to its input (`close` ends
it), `run_stop` ends it, and `reload` changes its code while it runs,
keeping its state: a session's, or a `tondo run --live` program's by
process ID or name. A change it cannot make in place says to start again.

## Debugging

`debug_start` debugs a `.to` file or project (built with `--debug`), a
debug executable or a running debug build, and gives a session; with
`live`, `reload` changes its code where it stopped. `debug` sends it one
command and gives the answers, the events and what the program printed:

```text
break src/main.to:12 [if count > 3 and name != ""] [hits 5] [log at {i}: {total / i}]
break fn parse   watch total   delete <id>   catch panic error   catch none
continue   step   next   finish   pause   wait
locals   print items[i + 1].name   print total / count   children items 100 50
set count 3   context   stack   frame 1   task 2   tasks   reload   restart
```

`locals` lists what the stopped line can use: its bindings, what a closure
captured, and those of the functions it is written in that are on the
stack, as a test's suites, each with where it is from; `context` the
program's contexts and the `with` that set each field. Conditions, `print`,
`watch` and the `{}` of a message take expressions: bindings and their
parts, literals, arithmetic, comparisons, `and`, `or`, `not`, and
`length()`, `isEmpty()`, `isSome()`, `isNone()`, `isOk()`, `isErr()`,
`contains()`, `startsWith()` and `endsWith()`; they read values and never
run the program's code. `watch` stops on the line after the one that
changed the value, in its frame or a call it makes; `catch error` stops
where an error starts, a `fail` or a `?` on what the standard library
failed with. `children` gives a page of a large value's parts from
`start`. `debug_stop` ends the session and its program.

## Observing and profiling

`ps` lists the programs started with `TONDO_DIAGNOSTICS=1` and the debug
builds; `counters`, `trace`, `dump` and `dump_read` look into one by
`program`, its process ID or name. `profile` profiles a `path` it runs, a
running `program`, a `test` or the benchmarks (`bench`), as `cpu`,
`memory` or `tasks`, and gives the functions and the program's lines that
take the most, each with its code.

## Resources

`tondo://guide/<topic>` is a topic of the guide, `tondo://skill` the
overview as an agent skill, and `tondo://api/<module>` a standard module's
declarations.
