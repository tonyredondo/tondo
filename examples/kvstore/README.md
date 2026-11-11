# kvstore: a TCP server around a pure function

A key-value store that speaks a small protocol of lines over TCP. Its
interest is in how it is put together: everything the server decides is one
function with no effects, which a test checks in every state it can reach,
and the network is a thin layer around it.

```text
kvstore/
  tondo.toml
  src/
    main.to                       listens until Ctrl-C
    protocol/protocol.to          a line into a `Command`, a `Reply` into a line
    protocol/protocol_test.to     with a fuzz test of the parser
    store/store.to                `apply(store, command)`: the whole logic
    store/store_test.to           examples, and a model of every state
    server/server.to              connections, each in a task, one shared store
  tests/server.to                 real clients over real sockets
```

## Run it

```sh
tondo run examples/kvstore -- 7070
tondo test examples/kvstore
```

```text
$ nc localhost 7070
SET name Ada Lovelace
OK
GET name
VALUE Ada Lovelace
INCR visits
VALUE 1
INCR visits 41
VALUE 42
INCR name
ERROR `name` does not hold a number
KEYS
KEYS name visits
FLY away
ERROR unknown command `FLY`
QUIT
BYE
```

## What to look at

**The protocol is two enums.** [`protocol.to`](src/protocol/protocol.to)
turns a line into a `Command` or a `ProtocolError`, and a `Reply` into a
line. `parse` uses `match` on the command's name and on the shape of its
arguments, `[key]` or `[key, amount]`, and every way a line can be wrong is
a variant with its message.

**The logic is one function.** [`store.apply`](src/store/store.to) takes a
store and a command and gives the next store and the reply. It reads no
clock, touches no socket and shares nothing, so the same inputs always give
the same answer. A store is a value: `apply` returns a new one, and the old
one is still what it was.

**A model of every state.** The store keeps a count of its numeric values,
updated as values change. That is the kind of redundant state that drifts.
[`store_test.to`](src/store/store_test.to) checks it with `std.model`: it
gives `model.machine` the function, the commands that can come over two
keys, and the rule, and the model applies every command in every state it
reaches, 16 states by 168 steps here. With the line that keeps the count
taken out of `without`, the test fails with the shortest way there:

```text
panic[P0008]: explicit-panic: `numbers counts the numeric values` does not hold
  start: Store { values: [:], numbers: 0 }
  Set { key: "a", value: "1" }: Store { values: ["a": "1"], numbers: 1 }
  Set { key: "a", value: "text" }: Store { values: ["a": "text"], numbers: 1 }
```

No example-based test in the file would have caught that one.

**The parser is searched.** `anyLineIsACommandOrAnError` runs with a few
seeds in a plain `tondo test`, and `tondo test --fuzz` looks for a line that
panics or breaks its rule.

**A task for each connection.** [`server.to`](src/server/server.to) accepts
in a loop and starts `spawn detached serve(connection, state)` for each
client. `serve` reads a line, waits up to a minute for it, and is written as
plain sequential code: nothing marks it as asynchronous, and a thousand idle
connections are a thousand waiting tasks, not threads.

**One lock, held for one step.** The store lives in a `sync.Mutex`.
`answer` does read, apply and write inside one `update`, so two connections
counting the same key cannot lose a count. [`tests/server.to`](tests/server.to)
proves it from outside: ten clients, ten increments each, and the answer is
100.

**Stopping.** `main` waits on `process.signals()`. Ctrl-C cancels it, the
wait ends, and the program exits with its tasks cancelled and their `defer`s
run.

## Going further

- Keep the store on disk: append each `Set` and `Delete` to a file with
  `fs.appendText`, and replay the file through `apply` when starting. Since
  `apply` is pure, replaying is the same code as serving.
- `TONDO_DIAGNOSTICS=1` in its environment, and `tondo blocked <name>` shows
  which connection waits for what.
