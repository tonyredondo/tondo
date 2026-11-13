# scripts: small programs in one file

A script is a `.to` file with no `main`: its statements run from the top.
`tondo <file.to>` compiles it, keeps the build, and runs it, so the second
run starts at once. These are the kind of thing one would write in a shell
or in Python, with types checked before the first line runs.

```sh
tondo examples/scripts/cal.to 2026 11
examples/scripts/cal.to 2026 11          # each starts with #!/usr/bin/env tondo
```

| Script | What it does | Shows |
| --- | --- | --- |
| [`wc.to`](wc.to) | counts lines, words and characters of files or of its input | reading files and standard input, a table from a record |
| [`biggest.to`](biggest.to) | the largest files under a directory | walking a tree, sorting, formatting numbers |
| [`dupes.to`](dupes.to) | files with the same content | a task for each file, hashing on every core |
| [`pick.to`](pick.to) | the part of a JSON document at a path | JSON of unknown shape, an exit code |
| [`csvsum.to`](csvsum.to) | adds up a column of a CSV for each value of another | CSV read a record at a time, maps, tuples |
| [`authors.to`](authors.to) | who wrote the last commits of a repository | running a program and reading what it prints |
| [`rerun.to`](rerun.to) | runs a command again when files change | watching a directory, a loop that never ends |
| [`cal.to`](cal.to) | the calendar of a month | dates |
| [`serve.to`](serve.to) | serves a directory over HTTP | a web server in four lines |
| [`ask.to`](ask.to) | asks a question and takes one key as the answer | the system's calls: a terminal's mode, a descriptor read |

## What they print

```text
$ tondo wc.to ../sitegen/site/content/*.md
┌───────┬───────┬────────────┬────────────────────────────────────────┐
│ lines │ words │ characters │ file                                   │
├───────┼───────┼────────────┼────────────────────────────────────────┤
│     6 │    19 │         96 │ ../sitegen/site/content/about.md       │
│    14 │    51 │        270 │ ../sitegen/site/content/hello-tondo.md │
│     9 │    40 │        227 │ ../sitegen/site/content/values.md      │
│    29 │   110 │        593 │ total                                  │
└───────┴───────┴────────────┴────────────────────────────────────────┘

$ tondo biggest.to ../linkcheck 5
┌────────┬─────────────────────────────────┐
│ size   │ file                            │
├────────┼─────────────────────────────────┤
│ 7.7 kB │ ../linkcheck/README.md          │
│ 6.5 kB │ ../linkcheck/src/crawl/crawl.to │
│ 5.7 kB │ ../linkcheck/tests/crawl.to     │
│ 2.5 kB │ ../linkcheck/src/main.to        │
│ 2.0 kB │ ../linkcheck/src/links/links.to │
└────────┴─────────────────────────────────┘

12 files, 27.9 kB in all

$ tondo dupes.to .
a/one.txt
b/copy.txt

3 files, 1 set with the same content

$ echo '{"name":"tondo","items":[{"name":"first","tags":["a","b"]}]}' | tondo pick.to 'items[0].name'
first

$ tondo csvsum.to sales.csv region amount
amount by region
┌───────┬──────┬────────┐
│ group │ rows │ total  │
├───────┼──────┼────────┤
│ east  │    2 │ 270.00 │
│ north │    3 │ 265.65 │
│ south │    2 │ 95.75  │
└───────┴──────┴────────┘

1 rows without a `region` or a number in `amount` were left out

$ tondo authors.to 5
┌─────────┬──────────────┬────────────────────────────┐
│ commits │ author       │ latest                     │
├─────────┼──────────────┼────────────────────────────┤
│       3 │ Ada Lovelace │ 2026-11-05 Change number 5 │
│       2 │ Alan Turing  │ 2026-11-04 Change number 4 │
└─────────┴──────────────┴────────────────────────────┘

$ tondo rerun.to notes -- ls notes
a.txt
--- ls notes: exit 0 in 1 ms
--- 1 changed, first notes/b.txt
a.txt
b.txt
--- ls notes: exit 0 in 1 ms

$ tondo cal.to 2026 11
   November 2026
Mo Tu We Th Fr Sa Su
                   1
 2  3  4  5  6  7  8
 9 10 11 12 13 14 15
16 17 18 19 20 21 22
23 24 25 26 27 28 29
30

$ tondo ask.to "Remove the build?" && echo removed
Remove the build? [y/n] y
removed
```

## What to look at

**A script is the whole language.** There is no smaller dialect for
scripts. `dupes.to` starts a task for each file with `spawn` and collects
them with a `Group`; `wc.to` declares a record and prints a table of it;
`pick.to` ends with an exit code. Each is checked as a whole before it
runs: a misspelt name or a wrong type stops it at the first line, not in
the middle of a directory half processed.

**Failing is the default, and it is tidy.** At the top of a script, `?`
passes a failure on, and the script ends with the error and a status that
is not zero. `fs.readText(file)?` in `wc.to` is all the error handling it
needs for a file that is not there. Where a failure deserves its own words,
as `git` not being run in a repository in `authors.to`, a `match` gives
them.

**A command is written as in a shell, and no shell runs it.**
`$(git log -n {wanted} --date=short)` in `authors.to` is split into a
program and its arguments when the script is compiled, and `wanted` is one
argument whatever it holds: nothing is expanded or split again, so a file
name with a space or a `$` in it is only a file name. A command that fails
is an error, as a function's is. `rerun.to` has its command as values, not
as text, so it uses the other form, `process.command(command[0],
...command[1:])`, and reads the exit code of a command that failed from the
error.

**The system is within reach.** `ask.to` takes one key without waiting for
Enter, which is a setting of the terminal: `unix.terminalMode` reads it,
the script clears two of its flags for the length of one `unix.read`, and
sets it back. The calls are those of C by their names, in `std.os.unix`,
safe to call and failing with the system's error. When its input is not a
terminal the script reads a line instead, so it works in a pipe too.

**The library a script needs is there.** JSON, CSV, hashes, dates, tables,
watching files and a web server are in the standard library, so a script is
one file with nothing to install, and stays one file.

**From script to program.** A script that grows gets a `main`, then a
directory with `tondo.toml`, modules and tests, without being rewritten:
the code is the same language all along. The other examples are where these
end up.
