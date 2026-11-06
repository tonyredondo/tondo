# Runbook: a slow request

A server answers late, and it matters which part of a request took the time.
With `TONDO_DIAGNOSTICS=1`, or in a session, a program answers what it serves.

```tondo file=server.to
import std.http
import std.time

fn lookup() {
    time.sleep(time.milliseconds(300))
}

fn main(): !http.HttpError {
    var app = http.App.new()
    app.get("/slow", () {
        lookup()
        "done"
    })
    app.listen(<port>)?
}
```

```mcp
run_start {"path": "server.to", "wait": 2}
blocked {"program": "<pid>"}
```

```sh
curl -s http://127.0.0.1:<port>/slow
```

`requests` gives those in flight and the last served, with what each took and
what it did: the tasks it went through and what it waited for.

```mcp
requests {"program": "<pid>"}
=> /slow
```

`logpoint` with `request` follows one kind of request on a line, without stopping
anything; `dump_trend` says which kinds grow slower over the dumps of a directory.
A request that does not end is a hang: `runbook-hang`.

```mcp
run_stop {"session": "<session>"}
```
