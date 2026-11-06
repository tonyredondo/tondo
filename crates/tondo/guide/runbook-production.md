# Runbook: failures in production

Many dumps come from many machines. Start from what is frequent and new, and
share nothing a program held.

```tondo file=app.to
import std.console
import std.env

fn charge(cents: Int): Int {
    100 / cents
}

fn main() {
    console.println("{charge(env.args().length())}")
}
```

```sh
tondo build app.to --output app
mkdir dumps
TONDO_DUMP=dumps ./app
TONDO_DUMP=dumps ./app
```

`dump_group` sorts them by failure, the most frequent first; `since` keeps those
new after a commit, as those a release brought.

```mcp
dump_group {"dir": "dumps"}
=> "count":2
=> app.to:5 in charge
```

`dump_issue` writes a group as an issue, and `dump_report` one dump as a page.

```mcp
dump_issue {"dir": "dumps", "signature": "<signature>"}
=> app.to:5
dump_report {"file": "<dump>", "output": "report.html"}
```

What goes outside goes through `dump_redact`: where the program was and how it
went stay, and what it held goes. Nothing added to dumps later gets out by itself.

```mcp
dump_redact {"file": "<dump>", "output": "shared.dump.json"}
dump_read {"file": "shared.dump.json", "view": "why"}
=> panic[P0003]
```

A core of the system, from a crash inside C or of the runtime, is `dump_core`:
it makes a dump of it. To go on from a failure: `runbook-crash`, and
`runbook-regression` for its test.
