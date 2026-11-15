# Runbook: a hang

A program or a test does not end, or stops answering. The goal is to know what
each task waits for. Do not add prints or sleeps: the tools ask the running
program. A program where every task waits needs none of this: it ends by
itself with `P0013`, and says where each one waits.

```tondo file=worker.to
import std.console
import std.time

fn beat() {
    for {
        time.sleep(time.milliseconds(50))
    }
}

fn fetch(answers: Receiver[Int]): Int {
    answers.receive() ?? 0
}

fn main() {
    let (send, answers) = channel[Int]()
    spawn detached beat()
    console.println("asking")
    console.println("{fetch(answers)}")
    send.send(1)
}
```

`run_start` keeps it running as a session, and gives its process, which the
tools below ask. `run` would wait for it to end.

```mcp
run_start {"path": "worker.to", "wait": 2}
run_output {"session": "<session>", "wait": 10}
=> asking
```

Each line of a session comes as it is written: `run_start` gives those so far,
and `run_output` what came after. A Release build writing to a pipe sends its
output when a buffer fills or it has nothing to do: while it computes, it may
have gone further than what shows.

`blocked` says at once what the tasks wait for and where, with who can wake
them.

```mcp
blocked {"program": "<pid>"}
=> worker.to:11 in fetch
=> a value of a channel that holds 0
```

A dump keeps it, to read with more detail or after the program is gone. `why`
points at what stands out, and `stacks` groups the tasks by where they are:
many tasks in one place is the usual shape of a hang.

```mcp
dump {"program": "<pid>", "output": "hung.dump.json"}
dump_read {"file": "hung.dump.json", "view": "stacks"}
=> worker.to:11 in fetch
```

`fetch` waits on a channel, and the only `send` comes after it, in the same
task. Send first, or from another task.

```edit worker.to
-    console.println("{fetch(answers)}")
-    send.send(1)
+    send.send(1)
+    console.println("{fetch(answers)}")
```

```mcp
run_stop {"session": "<session>"}
run {"path": "worker.to", "timeout": 300}
=> 1
```

With `beat` still running this ends with `main`, so the run ends too.

## The task that was to wake them

The cause is often not among the tasks that wait: the one that was to wake
them ended with an error, and nobody had read its result yet. The report of
`P0013` names it, with where it was spawned, and so do the views `why` and
`tasks` of a dump of a program that still runs.

```tondo file=report.to
import std.console
import std.fs

/// The report's lines, read by a task of its own.
fn load(path: String): Array[String] ! fs.FsError {
    fs.readText(path)?.lines()
}

fn main() {
    let (ready, started) = channel[Unit](1)
    let lines = spawn load("report.txt")
    // Waits for a word that only a load that went well would send.
    _ = started.receive()
    ready.send(())
    console.println("{lines.wait() ?? []}")
}
```

```mcp
run {"path": "report.to"}
=> panic[P0013]
=> task 2 ended with an error nobody read: err(fs.FsError.NotFound)
=> it was spawned at
```

There is no file, the load failed, and `main` waits for a word that will not
come. Read a task's result before waiting for what only its success sends.

## What else there is

A program no tool can reach, without `TONDO_DIAGNOSTICS`, still answers
`kill -QUIT`: it writes where each task is and ends.

A hang that comes only now and then, under load, is most often a wake lost: a
task waits, and what should wake it never reaches it. With `TONDO_WAKES=1` in
the program's environment, `blocked` and a dump give each waiting task's last
waits and wakes: when it parked, for what and where, which task handed it a
value, and what ended each wait. A task whose wait was claimed but that is
still parked lost its wake on the way; one whose last wait nothing claimed was
never woken. Without it the program pays nothing.

For a test that hangs, `test` takes `flags` with `--timeout <seconds>`;
`lock_order` finds locks taken in opposite orders, which only sometimes hang.
What a program that runs is doing, when it is slow rather than stopped:
`runbook-running`.
