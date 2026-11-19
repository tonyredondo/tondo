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
$ tondo counters todo --count 1
todo (pid 3640370), up 2.4 s

memory
  held                           121.5 KiB
  allocated                          0 B/s   total 1.5 MiB
  allocations                        0.0/s   total 3183
  objects                               60
  regions                        864.0 KiB
  regions_kept                         0 B
  region_allocations                 0.0/s   total 364

collector
  collections                        0.0/s   total 0
  freed                              0.0/s   total 0
  paused                              0.0%   total 0 us
  longest                             0 us

tasks
  live                                   2
  ready                                  0
  spawned                            0.0/s   total 7

profile
  samples                            0.0/s   total 0

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

grpc
  client.calls                       0.0/s   total 0
  client.active                          0
  client.failed                      0.0/s   total 0
  server.calls                       0.0/s   total 0
  server.active                          0
  server.failed                      0.0/s   total 0

sockets
  tcp                                    0
  listening                              1
  udp                                    0
  unix                                   0

descriptors
  files                                  2
  pipes                                  2
  total                                 12

processes
  running                                0
  started                            0.0/s   total 0

os
  cpu                                 0.0%   total 18.9 ms
  resident                        42.5 MiB
  threads                               54
```

One screen of what matters in a server: memory held and the rate it is
allocated at, the collector of cycles and how long it paused, tasks alive
and ready to run, the workers and how many are inside C, sockets, and the
requests of `std.http` and `std.grpc`. Without `--count` it prints the
screen again every second, as rates, until it is stopped; `--interval`
gives another pace.

- `tasks ready` that stays above zero is a program with more work than
  cores. `workers in_c` that stays high is tasks blocked in C calls.
- `memory held` that grows while `objects` does not is large values;
  both growing is a leak, and [dumps](#everything-at-once-dump) find it.
- `memory regions` is what regions hold now, the values a release build
  lets go of all at once when a scope ends
  ([Performance](performance.md#what-a-region-takes)), with the few chunks
  each thread keeps ready for its own; `regions_kept` is what the threads
  share, kept for the next regions; and `region_allocations` is how many
  values were made in them.
- `profile samples` counts the samples that a
  [profile](#where-the-time-goes-profile) or a trace took of the program's
  threads, so that the program itself, or whoever watches it, can tell
  that one is being taken and how far it got.
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

When it is in place the tool says so on its error stream, with `--json`
too, so that a script which then makes the line run waits for that first:

```text
$ tondo logpoint counts counts.to:9 "round {round}" --max 2
tondo: a logpoint on counts.to:9, in 1 place: it ends after 2 times
round 7 (as line 9 leaves it)
round 8 (as line 9 leaves it)
```

A message never holds a value that is not the binding's at that line. In a
Release build the optimized code may make a value after the place where the
line begins. The logpoint then takes it where it is first known, before
anything changes it, and the message says `(as line 9 leaves it)`; in the
JSON that value has `"from": "counts.to:10"`. A binding that the optimized
code makes and uses up between two lines reads `?`, and the tool says why:

```text
$ tondo logpoint counts counts.to:9 "round {round}, doubled {doubled}" --max 1
tondo: a logpoint on counts.to:9, in 1 place: it ends after 1 time
round 10, doubled ? (as line 9 leaves it)
tondo: `doubled` is not known here, as the optimized code makes its value later; a build with --debug knows every value
```

A logpoint can be put on any line because a Release build keeps a place for
one at each: a few bytes that do nothing until a logpoint is asked for. They
cost what it takes to pass them, which in most code is nothing to measure
and in the tightest is not: a function of a few instructions that calls
itself runs a quarter more of them. A build that needs every cycle leaves
the places out, with `--no-logpoints` or `logpoints = false` in the
`[build]` of `tondo.toml`
([Performance](performance.md#what-the-places-for-logpoints-cost)), and then
says so when it is asked for one:

```text
$ tondo logpoint counts counts.to:8 "round {round}"
tondo: this program has no places for a logpoint or a breakpoint: it was built with `--no-logpoints`, or with `logpoints = false` in the `[build]` of its tondo.toml; build it again without that to put one in it
```

A build with `--debug` has no places for a logpoint either, since it has the
debugger's probes instead: in `tondo debug <pid>`, `break counts.to:10 log
round {round}` and then `continue` write the same lines without stopping
there, with every value ([Debugging](debugging.md)).

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
| `--tasks` | where the tasks are, at once or when `--duration` ends: thousands of them, grouped by stack |
| `tondo profile -- <program>` | profiles a program from its start |
| `tondo profile show <file> --lines` | by line instead of by function |
| `tondo profile show <file> --asm` | the instructions of the hottest function, each with its share |
| `tondo profile show <file> --html <path>` | a flame graph, as one file |

A profile is in pprof's format, with the function and the line of every
frame, so `go tool pprof`, Speedscope and the services that read pprof
read it.

On macOS a profile of processor time has no samples of what a thread that
C made runs when it calls into the program, and a dump does not say where
such a thread is: there the profiler signals each thread, and one of C's
may get the signal as it ends. On Linux both have them.

A profile of the tasks is a picture of one moment too, and the program
stands still for the instant it is taken: at once for a program that runs,
or when `--duration` ends:

```text
$ tondo profile todo --tasks --output tasks.pb.gz --json
{"duration_ms":82,"ended":null,"format":"tondo-profile-0.1/1","kind":"tasks","lost":0,"ok":true,"profile":"tasks.pb.gz","samples":2,"stopped":true}
```

`stopped` says whether it did. A program that does not stop within its
wait, as one on a machine too loaded to give its threads their turn, is
profiled as it runs: a task that was running then is shown where the system
had it, which may be a call of the system's library with none of the
program's functions under it. The command then says so beside where it
wrote the file, `the program did not stop within its wait: tasks that were
running are shown where the system had them`, and so does the tool
`profile` of the MCP server, which gives `stopped` with each profile of
tasks. Asking again is the way to a picture that names the program's lines.

A program that `tondo profile` starts itself has only `main` at first, so
its tasks need a moment to be named:

```text
$ tondo profile --tasks -- ./todo
tondo: a program that has just started has only `main`: give `--duration <seconds>` to say when its tasks are to be seen, as `tondo profile --tasks --duration 1 -- <program>`
```

A task that is ready and has not run yet is in the picture too, where it
was spawned, and in the one a program takes of itself with
[`profile.tasks()`](../std/profile.md) as well. Three long sums started as
tasks, by a program held to one core:

```tondo
import std.console

/// A sum that takes a while.
fn busy(rounds: Int): Int {
    var total = 0
    for i in 0..rounds {
        total += i % 7
    }
    total
}

fn main() {
    let first = spawn busy(40000000000)
    let second = spawn busy(40000000000)
    let third = spawn busy(40000000000)
    console.println("{first.wait() + second.wait() + third.wait()}")
}
```

```text
$ TONDO_DIAGNOSTICS=1 taskset -c 0 ./backlog &
$ tondo profile backlog --tasks --output tasks.pb.gz
tondo: 4 tasks: tasks.pb.gz; `tondo profile show tasks.pb.gz` shows where; the program did not stop within its wait: tasks that were running are shown where the system had them

$ tondo profile show tasks.pb.gz
tasks of tasks.pb.gz, 4 in all over 0.26s
     flat  flat%       cum   cum%  function
        2  50.0%         3  75.0%  main  backlog.to
        1  25.0%         1  25.0%  busy  backlog.to
        1  25.0%         1  25.0%  tondo_runtime::tasks::join::finish
        0   0.0%         4 100.0%  tondo_runtime::tasks::trampoline
        0   0.0%         3  75.0%  tondo_main
        0   0.0%         1  25.0%  spawn busy  backlog.to
        0   0.0%         1  25.0%  tondo_rt_join_wait
by label:
        2  50.0%  state=ready
        1  25.0%  state=runs
        1  25.0%  state=waits
```

One sum runs, in `busy`, and `main` waits for it. The two that are `ready`
have not had the core yet, so they are counted in `main`, at the lines that
spawned them, which `--lines` names: `backlog.to:14` and `backlog.to:15`.
Tasks that pile up `ready` are work with no core to do it. This program,
busy in a loop of arithmetic, is also one that did not stand still for its
picture, and the command said so.

## Everything at once: dump

```text
$ tondo dump todo --output live.dump.json
todo (pid 962902), dumped on request, after 24.1 s
built from github.com/tonyredondo/tondo at 9e7b747b8f
signature: 04dbb707bf2a
3 of its files are not here as it was built: `dump read <file> sources`
memory: 96.9 KiB held, 57 objects, 24.2 MiB resident
tasks: 2 (2 waits)
  1 waits <std>/http/server.to:166 in http.askedOrEnded
  1 waits <std>/http/server.to:377 in http.serve
live memory, sampled: about 0 B
tondo: the dump is in live.dump.json; `tondo dump read live.dump.json` shows it

$ tondo dump read live.dump.json tasks
task 1 (main) waits at <std>/http/server.to:166 in http.askedOrEnded
    <std>/http/server.to:166 in http.askedOrEnded
    <std>/http/server.to:148 in http.Server.wait
    <std>/http/server.to:30 in http.App.listen
    src/main.to:14 in main

task 2 (http.serve) waits at <std>/http/server.to:377 in http.serve
    <std>/http/server.to:377 in http.serve
    <std>/http/server.to:370 in http.serveThenSignal
    <std>/http/server.to:45 in spawn http.serveThenSignal
  spawned at <std>/http/server.to:45 in http.App.start
```

A dump of a running program is its state at one moment, for which it
stands still an instant and then goes on: every task with its stack and
what it waits for, the counters, and the live memory by the line that
allocated it. The command prints a summary of it, and `dump read` shows
each part. It is the
same file a crash leaves, read with the same commands
([Debugging](debugging.md#a-failure-somewhere-else-dumps)).

The memory is given for the heap and for regions apart. Of a program that
filled an array of two hundred thousand numbers which never leaves `main`,
and waits, run with `TONDO_HEAP_RATE=1` so that every allocation is in the
dump:

```text
$ tondo dump read holds.dump.json heap
live memory: about 146.9 KiB, from samples of one allocation every 1 B on average
       BYTES    OBJECTS  SITE                        TYPE
    73.7 KiB         20  outside the program's code  other
    71.9 KiB        168  holds.to:11 in main         String?
     1.4 KiB          5  holds.to:9 in main          Unit

       BYTES    OBJECTS  BY TYPE
    73.7 KiB         20  other
    71.9 KiB        168  String?
     1.4 KiB          5  Unit

in regions: about 1.5 MiB
       BYTES    OBJECTS  SITE                        TYPE
     1.5 MiB          1  holds.to:7 in main          Array[Int]
```

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

A program that fails leaves its dump inside its container, and says where
it is and how it leaves. `docker cp` takes it out, from a container that
has stopped too, and it is read on any machine, of another architecture as
well:

```text
$ docker run --name orders-1 orders
panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
 --> orders.to:7:39
stack:
  total  orders.to:7:39
  main   orders.to:12:23
tondo runtime: a dump of the program is in /.cache/tondo/dumps/orders-1.dump.json
tondo runtime: it leaves the container with `docker cp <container>:/.cache/tondo/dumps/orders-1.dump.json .`, or as it is written with TONDO_DUMP on a volume, or in the logs with TONDO_DUMP=stderr
$ docker cp orders-1:/.cache/tondo/dumps/orders-1.dump.json .
$ tondo dump read orders-1.dump.json locals
orders.to:7 in total
order: Order = Order { id: 7, lines: [10, 20] }
```

A container removed as it ends takes its dump with it, and one whose file
system is read-only cannot write it, which the program says once:

```text
tondo runtime: no dump: /.cache/tondo/dumps cannot be made (Read-only file system (os error 30)); give TONDO_DUMP `stderr`, which writes it into the logs, a directory the program can write, as a volume of its container, or `off`
```

So a service's container gets a volume for its dumps:

```text
$ docker run --read-only --tmpfs /dumps -e TONDO_DUMP=/dumps orders
...
tondo runtime: a dump of the program is in /dumps/orders-1.dump.json
tondo runtime: it is on a tmpfs, which `docker cp` does not reach and which goes with the container; TONDO_DUMP on a volume, or TONDO_DUMP=stderr into the logs, keeps the next one
```

A tmpfs, as here, goes with the container, as the program says; a named
volume or a mounted directory stays.

Or the dump goes where the container's output goes. With
`TONDO_DUMP=stderr` the program writes it into its error output, compressed
and as text between two marks, and `tondo dump read` takes the log:

```text
$ docker run --name orders-2 -e TONDO_DUMP=stderr orders
panic[P0001]: bounds: index 2 is out of bounds for an array of length 2: valid indices are 0 to 1, and -2 to -1 from the end
 --> orders.to:7:39
stack:
  total  orders.to:7:39
  main   orders.to:12:23
tondo runtime: a dump of the program follows; `tondo dump read <log>` reads it from the log
-----BEGIN TONDO DUMP-----
H4sIAAAAAAAE/+VY3XLqNhB+FY2uHWpjGwMXfYX2Ps1kFFsGFSO5kpyfYXj3fpJlcIiTQzid6cW5
...
-----END TONDO DUMP-----
$ docker logs orders-2 > orders.log 2>&1
$ tondo dump read orders.log locals
orders.to:7 in total
order: Order = Order { id: 7, lines: [10, 20] }
```

This asks nothing of the file system, and the logs are what a platform
keeps of a container that is gone: `kubectl logs`, the log of a CI job.
`tondo dump read` reads the last dump of a log, and passes over what stands
before each line, as the time that `docker logs -t` and GitHub Actions
write.

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
