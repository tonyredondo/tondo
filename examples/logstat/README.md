# logstat: a command-line tool

Reads the log of a server and says what its requests were: which routes are
busy, which requests were slow, how many failed. It shows how a command
line is a type, how several files are read at once, and how a parser is
tested with inputs nobody thought of.

```text
logstat/
  tondo.toml
  sample.log                      a log to try it on
  src/
    main.to                       the command line: an enum of subcommands
    entries/entries.to            a line of the log into an `Entry`, or why not
    entries/entries_test.to       its tests, one of them a fuzz test
    report/report.to              what the entries add up to
    report/report_test.to
```

## Run it

```sh
cd examples/logstat
tondo run -- routes sample.log
tondo run -- slow sample.log --over 40ms
tondo run -- status sample.log
tondo test
tondo build --output logstat      # and then ./logstat routes access.log
```

```text
$ logstat routes sample.log
skipped 1 lines that are not entries
┌─────────────┬──────────┬────────┬─────────┬────────┐
│ route       │ requests │ errors │ slowest │ mean   │
├─────────────┼──────────┼────────┼─────────┼────────┤
│ /users/:id  │        4 │      0 │ 14 ms   │ 11 ms  │
│ /orders     │        3 │      1 │ 912 ms  │ 337 ms │
│ /orders/:id │        2 │      0 │ 21 ms   │ 12 ms  │
│ /health     │        1 │      0 │ 1 ms    │ 1 ms   │
│ /search     │        1 │      0 │ 340 ms  │ 340 ms │
└─────────────┴──────────┴────────┴─────────┴────────┘

$ logstat slow sample.log --over 40ms
skipped 1 lines that are not entries
┌──────────────────────┬───────────────────┬────────┬────────┐
│ at                   │ request           │ status │ took   │
├──────────────────────┼───────────────────┼────────┼────────┤
│ 2026-11-11T10:00:04Z │ POST /orders      │    500 │ 912 ms │
│ 2026-11-11T10:00:08Z │ GET /search?q=tea │    200 │ 340 ms │
│ 2026-11-11T10:00:07Z │ POST /orders      │    201 │ 51 ms  │
│ 2026-11-11T10:00:02Z │ POST /orders      │    201 │ 48 ms  │
└──────────────────────┴───────────────────┴────────┴────────┘

$ logstat status sample.log
skipped 1 lines that are not entries
2xx      9 ##############################
4xx      1 ####
5xx      1 ####
```

## What to look at

**The command line is a type.** In [`main.to`](src/main.to), an enum is the
subcommands and a record is the options of each. A field is an option
unless `@cli.Argument` makes it positional, its type says what it accepts,
its default is the option's, and its `///` is its help:

```text
pub type Slow = {
    /// The files of the log.
    @cli.Argument
    files: Array[String]
    /// The time a request may take, as `250ms` or `2s`.
    over: time.Duration = time.milliseconds(250)
}
```

```text
$ logstat slow --help
The requests that took longer than a limit.

Usage: logstat slow [options] [files...]

Arguments:
  <files>  The files of the log.

Options:
      --over <over>  The time a request may take, as `250ms` or `2s`. [default: 250ms]
  -h, --help         Shows this help.

$ logstat rutes sample.log
error: unknown command `rutes`; did you mean `routes`?
```

`cli.parse[Logstat]()` gives the enum, and a `match` runs the command. A
wrong line never reaches the program: it ends with status 2 and the usage.

**A table is an array of records.** `cli.table(rows)` makes a column of
each field, with numbers to the right. `cli.markup("[yellow]...[/]")` styles
text, and writes no codes when the output is not a terminal.

**Errors that say why.** [`entries.parse`](src/entries/entries.to) returns
`Entry ! ParseError`, an enum with a case for each thing wrong with a line.
The program skips such lines and counts them; a caller that cared could
tell them apart.

**Files read at once.** `readAll` starts a task for each file with `spawn`,
and a `Group` gives what they read in the order the files were named. The
first file that cannot be read ends the others. Nothing in `read` says it
runs in a task.

**A pattern compiled once.** The regular expression that turns `/users/7`
into `/users/:id` is the initial value of a context's field, computed the
first time it is read and shared by every task. There is no global and
nothing to set up in `main`.

**A test that searches.** `anyLineIsReadOrRefused` says that reading a line
never panics, whatever the line. `tondo test` runs it with its seeds;
`tondo test --fuzz --fuzz-time 5` tries lines of its own:

```text
test logstat::unit::entries::anyLineIsReadOrRefused ... ok
  parse  1.0M inputs in 0:04, corpus 7, coverage 1%
```

A line that broke it would be reduced, saved under `fuzz/`, and run by every
later `tondo test`.
