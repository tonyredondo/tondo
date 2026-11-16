# Runbook: what a running program is doing

A program runs, it is not as fast as it should be, or nobody is sure what it
is at, and stopping it is not an option. A program started with
`TONDO_DIAGNOSTICS=1` in its environment answers questions as it runs, and
pays nothing until one is asked; a session of `run_start` and a debug build
answer too.

```tondo file=mailer.to
import std.console
import std.diagnostics
import std.time

type Job = { id: Int, size: Int }

fn send(job: Job) {
    time.sleep(time.milliseconds(job.size))
}

fn main() {
    let sent = diagnostics.counter("mails sent")
    console.println("sending")
    for id in 1..=20000 {
        let job = Job { id, size: if id % 10 == 0 { 200 } else { 5 } }
        send(job)
        sent.add()
    }
}
```

```mcp
run_start {"path": "mailer.to", "wait": 2}
=> sending
```

`ps` lists the programs that can be asked. The other tools take one as
`program`, by its process ID or its name.

```mcp
ps {}
=> mailer
```

## How much, and how fast

`counters` gives the runtime's counters, memory, tasks, sockets, requests,
processor, and those the program keeps with `std.diagnostics`, each with its
rate a second. `count` and `interval` take several readings.

```mcp
counters {"program": "<pid>"}
=> program.mails sent
```

Some forty mails a second, where each takes five milliseconds: something else
takes the time.

## Where it waits

`profile` on a running program samples it for `duration` seconds: `cpu` where
it computes, `memory` what it allocates, and `tasks` where its tasks wait,
which is the one for a program that is slow without being busy.

```mcp
profile {"program": "<pid>", "kind": "tasks", "duration": 1}
=> mailer.to:8 in send
```

## What changed between two moments

`dump` writes the whole program down without stopping it: every task with
where it is and what it holds, the counters, the live memory. `dump_diff`
compares two of them: the counters that moved, the memory by the place that
made it, the tasks that started and ended, and the bindings that changed.

```mcp
dump {"program": "<pid>", "output": "before.dump.json"}
counters {"program": "<pid>", "count": 2, "interval": 1}
dump {"program": "<pid>", "output": "after.dump.json"}
dump_diff {"first": "before.dump.json", "second": "after.dump.json"}
=> program.mails sent
```

`trace` records what the tasks do for some seconds, as a timeline that
https://ui.perfetto.dev opens, with each run of a task and each wait, and a
profile beside it.

```mcp
trace {"program": "<pid>", "duration": 1, "output": "mailer.json"}
=> "timeline"
run_stop {"session": "<session>"}
```

## What a line holds, as it goes by

A logpoint is the `print` one would add and deploy, without the deploy: it
says what a line holds each time the program gets there, and stops nothing.
It works on Linux, in a release build too, as the one in production is.

```sh linux
tondo build mailer.to --output mailer
TONDO_DIAGNOSTICS=1 ./mailer > mailer.log 2>&1 &
echo $! > mailer.pid
timeout 30 sh -c 'until grep -q sending mailer.log; do sleep 0.1; done'
```

`place` is the line, `text` what to say there, with expressions between
braces, and `if` what must hold for a time to count: here, only the mails
that take long. It ends after `max` times or `seconds`.

```mcp linux
logpoint {"program": "mailer", "place": "mailer.to:8", "if": "job.size > 100", "text": "mail {job.id} takes {job.size} ms", "max": 2, "seconds": 20}
=> takes 200 ms
```

Every tenth mail takes forty times what the others take, which is where the
rate went. Without `text` a logpoint gives every binding of the line; `request`
keeps the tasks that work for one kind of request, `stop` holds the task there
for some seconds, as a breakpoint of that task alone, and `dump` writes a dump
of the whole program the first time it counts. A build with `no_logpoints`, or
`logpoints = false` in the `[build]` of its `tondo.toml`, has no place for one,
and says so: the one in production is built without it.

```sh linux
kill $(cat mailer.pid)
```

## From here

- A program on another machine or in a container: `via`, as `ssh host` or
  `docker exec -i box`, in `ps`, `counters`, `trace`, `dump` and `debug_start`.
- A server's requests, each with what it waited for: `runbook-slow-request`.
- Nothing moves: `runbook-hang`. Memory grows: `runbook-memory`.
- Real breakpoints in a running release build, which stop one task while the
  others go on: `debug_start` with its process ID, in `runbook-debug`.
