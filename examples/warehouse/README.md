# warehouse: a gRPC service from a .proto file

The stock of a warehouse, served over gRPC, with the client that calls it
in the same program. The contract is a `.proto` file among the sources: the
compiler reads it, and the service is a trait to implement. It has the four
kinds of call that matter in practice: one that asks, one that changes, one
where the client sends a stream and one where the server does. About 330
lines of Tondo and 50 of Protocol Buffers; gRPC, HTTP/2 and the command line
came with `tondo`.

```text
warehouse/
  tondo.toml
  src/
    main.to                       the command line: the server, and a client
    api/stock.proto               the contract: messages and the service
    shelves/shelves.to            the service: the trait of the .proto, implemented
    shelves/shelves_test.to       its calls, tested as plain functions
    server/server.to              the server: the key, the log, health, reflection
  tests/grpc.to                   the calls over the wire, against a real server
```

## Run it

```sh
cd examples/warehouse
WAREHOUSE_KEY=s3cret tondo run -- serve &
export WAREHOUSE_KEY=s3cret
tondo run -- get TEA-001
tondo test
```

```text
$ warehouse get TEA-001
TEA-001  Green tea, 100 g: 40 available

$ warehouse get SOFA-01
NotFound: there is no item `SOFA-01`

$ warehouse reserve CUP-007 2
CUP-007  Porcelain cup: 1 available

$ warehouse reserve CUP-007 2
FailedPrecondition: only 1 of `CUP-007` left

$ warehouse restock CUP-007=12 TEA-001=60
restocked 2 items with 72 units
```

Without the key, a call that changes the shelves is refused:

```text
$ WAREHOUSE_KEY= warehouse reserve CUP-007 2
Unauthenticated: this call needs the warehouse's key
```

`warehouse watch CUP-007 TEA-001` in another terminal printed this while
the commands above ran, a line as each thing happened:

```text
TEA-001  40 available (0)
CUP-007  3 available (0)
CUP-007  1 available (-2)
CUP-007  13 available (+12)
TEA-001  100 available (+60)
```

The same server answers a plain `POST` with JSON, with no client and no
`.proto` at hand:

```text
$ curl -H 'content-type: application/json' -d '{"sku": "CUP-007"}' \
       localhost:50051/warehouse.v1.StockService/Get
{"sku":"CUP-007","name":"Porcelain cup","available":"13"}

$ curl -H 'content-type: application/json' -d '{}' localhost:50051/grpc.health.v1.Health/Check
{"status":"SERVING"}
```

and its log has a line for each call:

```text
2026-11-11T23:39:58.67240037Z INFO call method=/warehouse.v1.StockService/Get peer=127.0.0.1:51396 code=Ok ms=0
2026-11-11T23:39:58.676678181Z INFO call method=/warehouse.v1.StockService/Get peer=127.0.0.1:51400 code=NotFound ms=0
2026-11-11T23:39:58.681007141Z INFO call method=/warehouse.v1.StockService/Reserve peer=127.0.0.1:51410 code=Unauthenticated ms=0
2026-11-11T23:39:59.187559255Z INFO call method=/warehouse.v1.StockService/Reserve peer=127.0.0.1:51426 code=Ok ms=0
```

## What to look at

**The `.proto` file is a module.** [`stock.proto`](src/api/stock.proto) sits
among the sources, and `import warehouse.api.stock` is all it takes to use
it. There is no generator to install, no generated code to commit, and no
step of the build to forget: its messages are records (`stock.Item`), its
service is a trait (`stock.StockService`), and it comes with a client
(`stock.StockServiceClient`) and a function that makes a server's service of
a value that implements the trait (`stock.stockService(value)`).

**A call is a method.** In [`shelves.to`](src/shelves/shelves.to), each
`rpc` of the service is a method of `impl stock.StockService for Shelves`,
and its shape says what kind of call it is:

```text
fn get(self, request: stock.GetItem): stock.Item ! grpc.Status
fn restock(self, input: grpc.Stream[stock.Units]): stock.Restocked ! grpc.Status
fn watch(self, request: stock.WatchItems, out: Sender[stock.Change]): !grpc.Status
```

A stream from the client is something to iterate, a stream to the client is
a channel to send on, and a failure is a `grpc.Status` in the result, with
its code and its message. Forgetting a method, or giving one the wrong
types, does not compile.

**Check and change in one step.** `reserve` reads how many units there are
and takes them inside one `self.state.update(...)`. The test
`ordersForTheLastUnitsCannotAllGetThem` starts five orders for two cups at
once and holds that exactly two get them, in every order of the five tasks
that `tondo test` tries.

**A stream that waits for news.** `watch` sends what there is and then
every change, until the client goes. It takes one copy of the state, which
is all of it at one moment, so what it sends first and where it goes on from
cannot disagree. Then it waits in `updateWhen` for a change it has not seen,
with no polling. When the client disconnects the call is cancelled, the wait
ends, and the method is gone: there is nothing to unsubscribe.

**The service is tested as plain functions.**
[`shelves_test.to`](src/shelves/shelves_test.to) calls `shelves.reserve(...)`
and reads the `grpc.Status` it fails with, with no server. Even the stream
is tested that way: `spawn shelves.watch(request, out)` and a channel to
receive from.

**One place for what every call needs.** In
[`server.to`](src/server/server.to), `onCall` wraps each call of the
service: it refuses the ones that change the shelves without the key, in
the call's metadata, and writes the line of the log with the code the call
ended with. The methods of the service know nothing of either.

**Deadlines travel.** The client's `get` is called inside
`within time.seconds(5) { ... }`. The time left goes with the call, and the
server stops working on it when it passes. In
[`tests/grpc.to`](tests/grpc.to), `aDeadlineEndsAStream` puts a deadline
around a stream that has no end of its own.

**More than gRPC on the same port.** The routes that answer gRPC over HTTP/2
also answer gRPC-Web, for browsers, and Connect, which is what the `curl`
above speaks. `grpc.Health()` answers the health checks that load balancers
make, and `grpc.reflection(...)` lets `grpcurl` list and call the service
with no `.proto` file.

**A command line of options and subcommands.** `Warehouse` in
[`main.to`](src/main.to) is a record with the options every command shares
and a field of an enum, whose variants are the subcommands with their own
arguments. `--key` is read from `WAREHOUSE_KEY` when it is not given.

## Going further

- `tondo requests <name>` follows each gRPC call of a server run with
  `TONDO_DIAGNOSTICS=1` through its tasks, and `tondo counters <name>` shows
  `grpc.server.calls`, how many run and how many failed.
- `stock.StockServiceClient.on(grpc.connect("http://a:50051,b:50051",
  balancing: grpc.Balancing.RoundRobin)?)` spreads the calls over several
  servers.
- With TLS, `listen(port: 443, tls: some(http.Tls.files("cert.pem",
  "key.pem")))` on one side and an `https://` address on the other.
