# A running program

A server is not debugged by stopping it. This chapter is about the
commands that look inside a program while it runs: what it holds, what it
is doing, what it is waiting for, and where its time goes. They work on a
Release build, the one that ships, and cost nothing until they are asked
for. The runs shown are of the [`todo-api`](../../examples/todo-api)
example, with what they printed.

- [Turning it on](#turning-it-on)
- [What is running: ps](#what-is-running-ps)
- [What it holds: counters](#what-it-holds-counters)
- [What it is doing: requests](#what-it-is-doing-requests)
- [What it waits for: blocked](#what-it-waits-for-blocked)
- [What a line holds: logpoint](#what-a-line-holds-logpoint)
- [Where the time goes: profile](#where-the-time-goes-profile)
- [Everything at once: dump](#everything-at-once-dump)
- [A timeline: trace](#a-timeline-trace)
- [On another machine](#on-another-machine)
- [In a container](#in-a-container)
- [Changing it while it runs](#changing-it-while-it-runs)
- [From inside the program](#from-inside-the-program)

## Turning it on

```sh
TONDO_DIAGNOSTICS=1 ./todo 8080
```

A program started with `TONDO_DIAGNOSTICS=1` in its environment can be
watched, by its process ID or by its name. Without it the program opens
nothing and counts nothing: there is no port, no agent and no cost in a
program nobody is looking at. With it, the commands below reach it from
the same machine.

## What is running: ps

```text
$ tondo ps
PID          UPTIME  PROGRAM
3576673     1.0 s  /tmp/todo 8791 sqlite::memory:
```

Every command after this takes that process ID, or the program's name.

## What it holds: counters

```text
$ tondo counters todo
todo (pid 3576673), up 2.1 s

memory
  held                           108.4 KiB
  allocated                          0 B/s   total 617.6 KiB
  allocations                        0.0/s   total 2567
  objects                               60

collector
  collections                        0.0/s   total 0
  freed                              0.0/s   total 0
  paused                              0.0%   total 0 us
  longest                             0 us

tasks
  live                                   2
  ready                                  0
  spawned                            0.0/s   total 7

workers
  total                                 32
  idle                                  31
  in_c                                   0
  replaced                           0.0/s   total 0
  c_calls                            0.0/s   total 0

timers
  pending                                0

net
  connections                        0.0/s   total 0
  read                               0 B/s   total 0 B
  written                            0 B/s   total 0 B

http
  client.requests                    0.0/s   total 0
  server.requests                    0.0/s   total 5
  server.active                          0
```

One screen of what matters in a server: memory held and the rate it is
allocated at, the collector of cycles and how long it paused, tasks alive
and ready to run, the workers and how many are inside C, sockets, and the
requests of `std.http` and `std.grpc`. `--interval 1` prints it again every
second, as rates.

- `tasks ready` that stays above zero is a program with more work than
  cores. `workers in_c` that stays high is tasks blocked in C calls.
- `memory held` that grows while `objects` does not is large values;
  both growing is a leak, and [dumps](#everything-at-once-dump) find it.
- A program adds its own with `std.diagnostics`: a counter or a gauge it
  declares shows here beside the runtime's.

## What it is doing: requests

```text
$ tondo requests todo
no request in flight
GET /tasks/9 took 0 us
  0 ms  task 7 takes it
GET /tasks took 0 us
  0 ms  task 6 takes it
POST /tasks took 0 us
  0 ms  task 5 takes it
```

Each HTTP and gRPC request, with the tasks that worked for it and when
each began. A request that fans out, to a database and two other services
in tasks of their own, shows as that tree, and one that is slow shows which
branch it is waiting in. No tracing library was added to the program for
this: the runtime knows which request each task belongs to.

## What it waits for: blocked

```text
$ tondo blocked todo
task 2 at <std>/http/server.to:53 in http.drainWhenAsked has waited 7.6 s for a value of a channel that holds 0 of 16
the longest wait: task 2, 7.6 s at <std>/http/server.to:53 in http.drainWhenAsked, for a value of a channel that holds 0 of 16
```

Every task that waits, where, for how long, and for what: a value of a
channel, a lock and the task that holds it, a `Join`, a timer. Here the one
waiting task is the server's own, waiting to be told to shut down, which is
what an idle server should show.

It says who waits for whom, so a deadlock reads off as a chain that comes
back to where it started. A program whose every task waits does not hang:
it ends by itself with `P0013`, and says where each one waited.

## What a line holds: logpoint

```text
tondo logpoint todo src/store/store.to:48 "finding {id}"
```

A logpoint prints a message each time a running program passes a line,
with the values of its bindings in the message. It needs no new build and
no restart, it does not stop the program, and it works in a Release build:
the line to print is put into the running code and taken out again.

| | |
| --- | --- |
| `--if <condition>` | only when it holds |
| `--request <text>` | only in requests whose line holds the text |
| `--max <n>`, `--for <seconds>` | ends after so many times, or so long |
| `--dump <file>` | writes a dump the first time it is reached |
| `--stop <seconds>` | holds the task there for a moment, to look |

It is the `print` one would add and deploy, without the deploy. It works on
Linux.

## Where the time goes: profile

```text
$ tondo profile todo --cpu --duration 2 --output cpu.pb.gz
tondo: 94 samples of 1 ms of processor time: cpu.pb.gz; `tondo profile show cpu.pb.gz` shows where

$ tondo profile show cpu.pb.gz --top 300
cpu of cpu.pb.gz, 94ms in all over 2.03s
     flat  flat%       cum   cum%  function
     20ms  21.3%      20ms  21.3%  libc.so.6+0xa08f2
      7ms   7.4%       7ms   7.4%  shutdown
      6ms   6.4%       6ms   6.4%  syscall
      4ms   4.3%       4ms   4.3%  _mi_theap_malloc_zero
      2ms   2.1%       4ms   4.3%  tondo_rt_array_release
      2ms   2.1%       2ms   2.1%  sqlite3VdbeExec
      2ms   2.1%       2ms   2.1%  tondo_rt_str_release
      1ms   1.1%       3ms   3.2%  sql.readValue  <std>/sql/sqlite.to
      1ms   1.1%       2ms   2.1%  closure in sql.Row.next  <std>/sql/databases.to
      ...
      0ns   0.0%      51ms  54.3%  http.converse  <std>/http/server.to
      0ns   0.0%      30ms  31.9%  closure in http.App.pipeline  <std>/http/app.to
      0ns   0.0%      29ms  30.9%  api.traced  src/api/api.to
      0ns   0.0%      15ms  16.0%  closure in api.routes  src/api/api.to
      0ns   0.0%       8ms   8.5%  store.find  src/store/store.to
```

Two seconds of a server answering a few hundred requests: 94 ms of
processor in all, most of it in the system and in SQLite. The program's own
code took no sample by itself, so it is far down the list, which `--top`
lengthens: `api.traced` and the handlers have what they called under them
in the `cum` column.

| | |
| --- | --- |
| `--cpu` | where processor time goes |
| `--memory` | what is allocated, and what is held, by the line that made it |
| `--tasks` | where the tasks are: thousands of them, grouped by stack |
| `tondo profile -- <program>` | profiles a program from its start |
| `tondo profile show <file> --lines` | by line instead of by function |
| `tondo profile show <file> --asm` | the instructions of the hottest function, each with its share |
| `tondo profile show <file> --html <path>` | a flame graph, as one file |

A profile is in pprof's format, with the function and the line of every
frame, so `go tool pprof`, Speedscope and the services that read pprof
read it.

## Everything at once: dump

```text
$ tondo dump todo --output live.dump.json
tondo: the dump is in live.dump.json; `tondo dump read live.dump.json` shows it

$ tondo dump read live.dump.json tasks
task 1 (main) waits at <std>/http/server.to:358 in http.serve
    <std>/http/server.to:358 in http.serve
    <std>/http/server.to:33 in http.App.listen
    src/main.to:14 in main

task 2 (http.drainWhenAsked) waits at <std>/http/server.to:53 in http.drainWhenAsked
  waited 23.8 s for a value of a channel that holds 0 of 16
    <std>/http/server.to:53 in http.drainWhenAsked
```

A dump of a running program is its state at one moment, written without
stopping it: every task with its stack and what it waits for, the
counters, and the live memory by the line that allocated it. It is the
same file a crash leaves, read with the same commands
([Debugging](debugging.md#a-failure-somewhere-else-dumps)).

Two dumps a while apart answer the question about a leak: `tondo dump diff
before.json after.json` says what grew, by place, and `tondo dump trend
<dir>` does it over many.

## A timeline: trace

```text
tondo trace todo --duration 5 --output trace.json
```

`trace` records what each task did over a few seconds, when it ran, on
which worker, what it waited for, as a file that Perfetto opens as a
timeline. It answers what a profile cannot: not where the time went in
total, but in which order things happened and who was waiting meanwhile.

## On another machine

`ps`, `counters`, `trace`, `profile` and `dump` take `--via`, a command
that reaches the other side, where `tondo` is installed too:

```text
tondo counters --via "ssh api-3" todo
tondo dump --via "kubectl exec -i api-7f9 --" todo --output api.dump.json
```

Nothing listens on the network for this. The connection is whatever the
command is, with the access it already has.

## In a container

A static executable is all an image needs, and nothing has to be added to
it for what follows. The sessions were run with Docker, with images made
`FROM scratch`.

A program that fails leaves its dump inside its container, and says where.
`docker cp` takes it out, from a container that has stopped too, and it is
read on any machine, of another architecture as well:

```text
$ docker run --name orders-1 orders
panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
 --> orders.to:7:39
stack:
  total  orders.to:7:39
  main   orders.to:12:23
tondo runtime: a dump of the program is in /.cache/tondo/dumps/orders-1.dump.json
$ docker cp orders-1:/.cache/tondo/dumps/orders-1.dump.json .
$ tondo dump read orders-1.dump.json locals
orders.to:7 in total
order: Order = Order { id: 7, lines: [10, 20] }
```

A container removed as it ends takes its dump with it, and one whose file
system is read-only cannot write it, which the program says once:

```text
tondo runtime: no dump: /.cache/tondo/dumps cannot be made (Read-only file system (os error 30)); give TONDO_DUMP a directory the program can write, as a volume or a tmpfs of its container, or `off`
```

So a service's container gets a volume for its dumps:

```text
$ docker run --read-only --tmpfs /dumps -e TONDO_DUMP=/dumps orders
...
tondo runtime: a dump of the program is in /dumps/orders-1.dump.json
```

A tmpfs, as here, goes with the container; a named volume or a mounted
directory stays.

A program that is running is reached from a container beside it that
shares its processes, which is what `kubectl debug --target` makes. That
container holds only a static `tondo` (`FROM scratch`, `COPY tondo /tondo`,
`ENTRYPOINT ["/tondo"]`), and the program's image holds none:

```text
$ docker run --rm -i --pid=container:ticker-1 --tmpfs /tmp tondo-tools ps
PID          UPTIME  PROGRAM
1     5.8 s  /ticker
$ docker run --rm -i --pid=container:ticker-1 --tmpfs /tmp tondo-tools counters 1 --count 1
ticker (pid 1), up 7.1 s

memory
  held                             2.5 KiB
  allocated                          0 B/s   total 1.0 MiB
...
$ docker run --rm -i --pid=container:ticker-1 --tmpfs /tmp tondo-tools debug 1
> break ticker.to:6
breakpoint 1 at ticker.to:6
> continue
task 1 stopped at ticker.to:6 in main (breakpoint 1)
> print counter > 0
counter > 0: Bool = true
```

The same commands work with every capability dropped from both containers
(`--cap-drop ALL`): the tools ask nothing of the system that a container
withholds.

The other way is `--via` through the container itself, with a `tondo`
copied into it:

```text
$ docker cp tondo ticker-1:/tondo
$ tondo dump ticker --via "docker exec -i -e PATH=/ ticker-1" --output ticker.dump.json
tondo: the dump is in ticker.dump.json; `tondo dump read ticker.dump.json` shows it
```

When there is none, the answer says what is missing and how to put it
there:

```text
tondo: `docker exec -i -e PATH=/ ticker-2 tondo relay ticker` did not reach the program: OCI runtime exec failed: exec failed: unable to start container process: exec: "tondo": executable file not found in $PATH; no `tondo` is where it reaches: put one there, as a static tondo copied into a container with `docker cp tondo box:/tondo`, reached with `--via "docker exec -i -e PATH=/ box"`
```

The program must have `TONDO_DIAGNOSTICS=1` or `TONDO_DUMP` in its
environment, as on any machine, and a directory for temporary files that
it can write: with none, it says that the tools cannot reach it.

## Changing it while it runs

In development, a program can take in new code without losing its state.
This is not for production, and a Release build has none of it.

```text
tondo run --watch            reload as files are saved
tondo run --live             a live build, which `tondo reload <name>` changes
tondo eval todo "store.list(db, none)"    what an expression gives in the running program
tondo values todo src/api/api.to          what each line left the last time it ran
```

A live build keeps running across an edit. Calls made afterwards run the
new code, a loop already running moves to its new body at its next
statement, and values of a type that gained or lost a field are converted.
A server keeps its connections and its data; a long computation keeps what
it has done. Under `--watch` a panic does not end the program: the task
waits, and when a fix is saved the function that panicked starts again.

`tondo values` is a debugger's view without the debugger: beside each line
of a file, the values it last produced, in a program that never stopped.

## From inside the program

The same information is available to the program itself, for an endpoint
of its own or a log line:

| | |
| --- | --- |
| [`std.diagnostics`](../std/diagnostics.md) | the counters above as values, counters and gauges of the program's own, the stacks of its tasks |
| [`std.profile`](../std/profile.md) | a profile taken by the program, with labels on the work it measures |
| [`std.log`](../std/log.md) | structured logs, with the fields of the request's context on every line |

Next: [Performance](performance.md).
