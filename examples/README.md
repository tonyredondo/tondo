# Examples

Programs of the size one meets at work, each a project with its tests and a
README that shows it running and says what to look at in its code. They are
part of the repository's tests: every one is formatted, compiles without a
warning and passes its own tests on each change to the compiler.

| Example | What it is | What it shows |
| --- | --- | --- |
| [`todo-api`](todo-api) | a JSON API over SQLite | routes, typed handlers, validation, `std.sql`, migrations in the executable, a context for the request, tests over HTTP |
| [`logstat`](logstat) | a command-line tool that reads logs | subcommands from types, parsing with errors as values, tables, a fuzz test |
| [`kvstore`](kvstore) | a key-value store over TCP | a protocol as enums, a pure core behind a lock, a model of it checked in every state, sockets |
| [`sitegen`](sitegen) | a static site generator | templates, YAML into a type, file systems as values, a task for each page |
| [`linkcheck`](linkcheck) | a concurrent link checker | channels, a group of workers, a rate limit, a deadline around everything, tests against real servers |
| [`chat`](chat) | rooms over WebSockets | state as a value behind one lock, waiting for a condition, bounded memory for slow readers, the page inside the executable |
| [`warehouse`](warehouse) | a gRPC service and its client | a `.proto` as a module, the four kinds of call, interceptors, deadlines, health and reflection |
| [`textkit`](textkit) | a library for C | `pub extern fn`, a struct by value, C's conventions for buffers and callbacks, the header the build writes, a C program that links it |
| [`notes`](notes) | an MCP server | tools from the methods of a type, schemas from records, validated arguments, resources and prompts |
| [`scripts`](scripts) | nine programs of one file | scripts with no `main`: files, JSON, CSV, processes, hashing in tasks, dates, a web server |

## Running them

```sh
tondo run examples/todo-api            # a program
tondo test examples/kvstore            # its tests
tondo examples/scripts/cal.to          # a script
```

Each README has the commands for its example, with what they print.

## By what you are looking for

| To see | Look at |
| --- | --- |
| An HTTP server | `todo-api`, `chat`, `scripts/serve.to` |
| An HTTP client | `linkcheck` |
| A database | `todo-api` |
| A command line | `logstat`, `warehouse`, `sitegen` |
| Tasks and channels | `linkcheck`, `sitegen`, `scripts/dupes.to` |
| Shared state | `kvstore`, `chat`, `warehouse` |
| Deadlines and cancelling | `linkcheck`, `warehouse`, `chat` |
| Errors as values | `logstat`, `kvstore`, `sitegen` |
| Tests over a real socket | `todo-api`, `kvstore`, `linkcheck`, `chat`, `warehouse`, `notes` |
| Tests of every order of tasks | `warehouse`, `sitegen` |
| Fuzzing and models | `logstat`, `textkit`, `kvstore` |
| Files and templates | `sitegen`, `notes`, `scripts` |
| JSON, YAML, CSV, Protocol Buffers | `todo-api`, `sitegen`, `scripts/csvsum.to`, `warehouse` |
| Files inside the executable | `todo-api`, `chat` |
| C | `textkit` |
| Running other programs | `scripts/authors.to`, `scripts/rerun.to` |

## Beside the examples

- [The language](../docs/language/README.md), a chapter at a time.
- [The standard library](../docs/std/README.md), a page for each module.
- `tondo new <name>` makes a project of your own, and `tondo guide` is the
  language on a few pages.
