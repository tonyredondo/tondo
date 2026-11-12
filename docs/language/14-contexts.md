# Contexts and deadlines

Some values belong to the work that is going on rather than to any one
function: the request being answered, the user who made it, the trace it is
part of, the time it has left. Passing them down through every function is
noise, and globals are worse. A `context` is Tondo's answer, and `within` is
the same idea for time.

- [The problem](#the-problem)
- [Declaring and reading a context](#declaring-and-reading-a-context)
- [with: changing a context for a block](#with-changing-a-context-for-a-block)
- [Contexts and tasks](#contexts-and-tasks)
- [What a whole program shares](#what-a-whole-program-shares)
- [Carrying contexts elsewhere](#carrying-contexts-elsewhere)
- [Contexts in logs and profiles](#contexts-in-logs-and-profiles)
- [within: a deadline for a block](#within-a-deadline-for-a-block)
- [Deadlines that add up](#deadlines-that-add-up)
- [Cleaning up after a deadline](#cleaning-up-after-a-deadline)
- [When to use a context](#when-to-use-a-context)

## The problem

A server answers a request. Twenty functions down, a query to the database
should carry the request's identifier in its log line. In most languages
there are two ways to get it there. One is to add a parameter to all twenty
functions, and to every function added later. The other is a global, which
stops working the moment two requests are answered at once.

A context is a value that every function in a piece of work can read, that
is different for each piece of work going on, and that nobody passes.

## Declaring and reading a context

```tondo
import std.console

context Request = {
    id: String = ""
    user: String? = none
}

fn audit(action: String): String {
    let who = Request.user.unwrapOr("anonymous")
    "[{Request.id}] {who}: {action}"
}

fn save(document: String): String {
    audit("saved {document}")           // nothing about the request is passed
}

fn main() {
    console.println(save("notes.txt"))
}
```

```text output
[] anonymous: saved notes.txt
```

A context is declared like a record whose fields all have initial values.
Any function reads a field by the context's name, `Request.id`. Until
something changes it, a field has its initial value.

A context is not a variable: there is nothing to assign to. It changes only
with `with`.

## with: changing a context for a block

`with Name { field: value }` changes fields of a context from that line to
the end of the block it is in. However the block ends, the context is as it
was before:

```tondo
import std.console

context Request = {
    id: String = ""
    user: String? = none
}

fn audit(action: String): String {
    let who = Request.user.unwrapOr("anonymous")
    "[{Request.id}] {who}: {action}"
}

fn handle(id: String, user: String): String {
    with Request { id, user: user }
    audit("opened the page")
}

fn main() {
    console.println(handle("r-17", "ada"))
    console.println(handle("r-18", "bo"))
    console.println(audit("outside any request"))
}
```

```text output
[r-17] ada: opened the page
[r-18] bo: opened the page
[] anonymous: outside any request
```

- The change reaches everything the block calls, at any depth, and nothing
  else. It never flows back to the caller: a function cannot change the
  context of whoever called it.
- Several fields go in one `with`. Several contexts are several lines, one
  under another, with no nesting.
- A field left out of a `with` keeps the value it had.
- A `with` inside a `with` changes it further, and undoes only its own
  change when its block ends.

```tondo
import std.console

context Trace = {
    span: String = "root"
    depth: Int = 0
}

fn report(): String {
    "{"  ".repeat(Trace.depth)}{Trace.span}"
}

fn query() {
    with Trace { span: "db.query", depth: Trace.depth + 1 }
    console.println(report())
}

fn handle() {
    with Trace { span: "http.request", depth: Trace.depth + 1 }
    console.println(report())
    query()
    console.println(report())
}

fn main() {
    console.println(report())
    handle()
    console.println(report())
}
```

```text output
root
  http.request
    db.query
  http.request
root
```

## Contexts and tasks

A task starts with the contexts of the place that spawned it. A request's
context therefore follows the request into every task started on its
behalf, and two requests answered at the same time each see their own:

```tondo
import std.console

context Request = {
    id: String = ""
}

fn part(name: String): String {
    "{name} of {Request.id}"
}

fn handle(id: String): Array[String] {
    with Request { id }
    var group = Group[String]()
    group.add(spawn part("header"))
    group.add(spawn part("body"))
    group.wait()
}

fn main() {
    let first = spawn handle("r-1")
    let second = spawn handle("r-2")
    console.println("{first.wait()} {second.wait()}")
}
```

```text output
[header of r-1, body of r-1] [header of r-2, body of r-2]
```

What a task does with its contexts afterwards is its own: a `with` in a task
is not seen by the task that spawned it, nor by its siblings.

## What a whole program shares

There are no global variables in Tondo. What a program sets up once and uses
everywhere, as a compiled pattern, a pool of connections or a cache, is a
context field whose initial value is computed: the first read computes it,
and every task then shares the result.

```tondo
import std.console
import std.regex
import std.sync

context Shared = {
    words: regex.Regex = regex.Regex.compile(r"[a-z]+")?
    hits: sync.Atomic[Int] = sync.Atomic(0)
}

fn isWord(text: String): Bool {
    _ = Shared.hits.add(1)
    Shared.words.isFullMatch(text).unwrapOr(false)
}

fn main() {
    console.println("{isWord("hello")} {isWord("h3llo")} {Shared.hits.load()}")
}
```

```text output
true false 2
```

- The initial value is computed once, when the program first reads it, not
  when it starts: a program that never reads `Shared.words` never compiles
  the pattern.
- A `?` that fails there panics at that read, and the next read tries again.
- Nothing has to be set up in `main` or passed around, and a library can
  have shared values of its own that its users never see.
- A value set with `with` at the top of `main` holds for the whole program,
  since everything runs inside `main`: that is how a program replaces a
  default, as a logger or a configuration.

## Carrying contexts elsewhere

Some work does not run under the code that asked for it: a message put in a
queue is handled later, by a worker that started long before. The contexts
can be captured and given back:

```tondo
import std.console

context Request = {
    id: String = ""
}

fn describe(text: String): String {
    "[{Request.id}] {text}"
}

fn worker(jobs: Receiver[(Contexts, String)], done: Sender[String]) {
    for (contexts, text) in jobs {
        with contexts                           // the sender's contexts, for this message
        done.send(describe(text))
    }
}

fn submit(jobs: Sender[(Contexts, String)], id: String, text: String) {
    with Request { id }
    jobs.send((Contexts.current(), text))
}

fn main() {
    let (jobs, inbox) = channel[(Contexts, String)](4)
    let (done, results) = channel[String](4)
    spawn worker(inbox, done)
    submit(jobs.copy(), "r-1", "resize the image")
    submit(jobs, "r-2", "send the email")
    console.println("{results.receive()} {results.receive()}")
}
```

```text output
some([r-1] resize the image) some([r-2] send the email)
```

`Contexts.current()` is every context as it is at that point, as a value
that can be stored and sent. `with captured` puts them back until the end of
the block. A function can also return contexts for its caller to adopt,
which is how a tracing library starts a span: `with tracer.startSpan("db")`.

## Contexts in logs and profiles

A field marked `@log("name")` is written in every line `std.log` writes
while it is set, and in the report of a panic. One marked `@label` labels
the samples of a profile, so that time can be divided by it.

```tondo
import std.log

context Request = {
    @log("request_id")
    id: String = ""
    @label
    route: String = ""
}

fn handle(id: String, route: String) {
    with Request { id, route }
    log.info("handling")                // carries request_id=...
}

fn main() {
    handle("r-17", "/users/:id")
}
```

`std.log` is itself a user of contexts: `log.Context` holds the logger that
`log.info` and the others write with, and a `with log.Context { logger }`
at the top of `main` replaces it for the whole program.

The debugger shows the contexts of a stopped task and the `with` that set
each field, and a dump keeps them for every task.

## within: a deadline for a block

`within duration { ... }` gives a block a time limit. When the time passes,
the block's next wait ends, its `defer`s run, the tasks it started are
cancelled, and the block gives an error:

```tondo
import std.console
import std.time

fn fetch(delay: Int): String {
    time.sleep(time.milliseconds(delay))
    "page"
}

fn main() {
    match within time.milliseconds(200) { fetch(1) } {
        ok(page) => console.println("got the {page}")
        err(timedOut) => console.println("{timedOut}")
    }
    match within time.milliseconds(50) { fetch(5000) } {
        ok(page) => console.println("got the {page}")
        err(timedOut) => console.println("{timedOut}")
    }
}
```

```text output
got the page
timed out after 50ms
```

- A block of type `T` becomes a `T ! time.TimedOut`. A block that already
  fails with `E` becomes `T ! (E | time.TimedOut)`, and `?` passes either on.
- The deadline is one more thing that follows the work: it holds for
  everything the block calls and for the tasks it starts, without any of
  them taking a timeout parameter. A detached task is not under it.
- Like cancelling, it acts at a wait. A computation that never waits is not
  stopped.
- The libraries know about it: a request made by `std.http` inside a
  `within` has that long, and a gRPC call sends the time it has left to the
  server.

## Deadlines that add up

Code can ask how long it has, and a deadline inside another can only be
earlier:

```tondo
import std.console
import std.time

fn step(name: String): String {
    match time.remaining() {
        some(left) => "{name} has at most {left.seconds() + 1} s"
        none => "{name} has no deadline"
    }
}

fn main() {
    console.println(step("outside"))
    let result = within time.seconds(30) {
        let outer = step("the request")
        let inner = within time.seconds(5) { step("the query") }
        let wider = within time.seconds(300) { step("a patient step") }
        "{outer} | {inner} | {wider}"
    }
    console.println("{result}")
}
```

```text output
outside has no deadline
ok(the request has at most 30 s | ok(the query has at most 5 s) | ok(a patient step has at most 30 s))
```

`time.deadline()` gives the moment and `time.remaining()` the time left, or
`none` when there is no deadline. Work that takes several steps uses them to
give up early, or to split what is left among its steps.

## Cleaning up after a deadline

When a task is cancelled, the waits in its `defer`s end at once: a cancelled
task should stop, not start waiting again. A cleanup that really has to
wait, as a library that sends what it has buffered before the program exits,
says how long it may take, with a `within` of its own:

```tondo
import std.console
import std.time

fn flush(): String {
    time.sleep(time.milliseconds(5))        // stands for sending what is left
    "flushed"
}

fn stop() {
    match within time.seconds(5) { flush() } {
        ok(done) => console.println(done)
        err(_) => console.println("gave up flushing")
    }
}

fn main() {
    defer stop()
    console.println("working")
}
```

```text output
working
flushed
```

A wait inside a `within` goes on until its own deadline even in a cancelled
task. So `defer tracer.stop()` is all a caller writes, and on `SIGTERM` the
program still sends its last data, within five seconds.

## When to use a context

Use a context for what belongs to the work in progress and would otherwise
be passed through functions that do not care about it:

- the request, the user, the tenant, the locale;
- the trace and the span;
- the logger, and what every log line should carry;
- shared values of a program or a library, through computed fields.

Do not use one for what a function needs in order to do its job. A function
that computes a price from a quantity takes the quantity as a parameter: a
context that it reads instead would hide its real input, and make it give
different answers for the same arguments. The test is whether the function's
result depends on it. What the result depends on is a parameter; what only
accompanies the work is a context.

Next: [Modules, packages and projects](15-modules.md).
