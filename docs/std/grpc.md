# std.grpc

gRPC over HTTP/2. A service of a `.proto` file gives a trait whose methods are its calls, a function that makes a server's `Service` of a value that implements it, which `App.grpc` serves, and a client. Messages are `std.protobuf`'s, a call's failure is a `Status`, and what a call carries besides, its metadata, its peer and its deadline, `call()` gives in the method.

```tondo
import std.grpc
import std.http

// A service of a `.proto` file is served the same way:
// `app.grpc(users.userService(value))`.
let health = grpc.Health()
var app = http.App()
app.grpc(health.service())
app.grpc(grpc.reflection([health.service()]))
app.listen(port: 50051)?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.grpc` shows the same in a terminal.

- [Functions](#functions): [`call`](#call), [`setHeader`](#setheader), [`setTrailer`](#settrailer), [`unary`](#unary), [`serverStream`](#serverstream), [`clientStream`](#clientstream), [`bidiStream`](#bidistream), [`connect`](#connect), [`reflection`](#reflection)
- [Code](#code): [`Code.number`](#codenumber), [`Code.of`](#codeof)
- [Status](#status): [`Status.from`](#statusfrom)
- [Call](#call-1)
- [Stream](#stream)
- [Service](#service): [`Service.onCall`](#serviceoncall), [`Service.onMessage`](#serviceonmessage), [`Service.compress`](#servicecompress)
- [Method](#method)
- [Direction](#direction)
- [ServerCall](#servercall): [`ServerCall.receive`](#servercallreceive), [`ServerCall.send`](#servercallsend)
- [Compression](#compression)
- [Balancing](#balancing)
- [Channel](#channel): [`Channel.onCall`](#channeloncall), [`Channel.onMessage`](#channelonmessage), [`Channel.unary`](#channelunary), [`Channel.serverStream`](#channelserverstream), [`Channel.clientStream`](#channelclientstream), [`Channel.bidiStream`](#channelbidistream)
- [Upload](#upload): [`Upload`](#upload-1), [`Upload`](#upload-2)
- [Duplex](#duplex): [`Duplex`](#duplex-1), [`Duplex.closeSend`](#duplexclosesend), [`Duplex`](#duplex-2), [`Duplex.headers`](#duplexheaders), [`Duplex.trailers`](#duplextrailers)
- [ServingStatus](#servingstatus)
- [Health](#health): [`Health`](#health-1), [`Health.set`](#healthset), [`Health.service`](#healthservice)

### Functions

#### call

```tondo
pub fn call(): Call
```

The call the running method serves; an empty one outside a method.

#### setHeader

```tondo
pub fn setHeader(name: String, value: String)
```

Sets metadata the server answers with, before its first message.

#### setTrailer

```tondo
pub fn setTrailer(name: String, value: String)
```

Sets metadata that follows the server's last message, with the status.

#### unary

```tondo
pub fn unary[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](exchange: ServerCall, method: fn(In): Out ! Status): !Status
```

A unary method: one request, one reply.

#### serverStream

```tondo
pub fn serverStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](exchange: ServerCall, method: fn(In, Sender[Out]): !Status): !Status
```

A method that sends a stream: one request, and replies as it sends them.

#### clientStream

```tondo
pub fn clientStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](exchange: ServerCall, method: fn(Stream[In]): Out ! Status): !Status
```

A method that receives a stream: the requests, and one reply.

#### bidiStream

```tondo
pub fn bidiStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](exchange: ServerCall, method: fn(Stream[In], Sender[Out]): !Status): !Status
```

A method that receives a stream and sends one.

#### connect

```tondo
pub fn connect(
    target: String,
    trust: String = "",
    certificate: String = "",
    key: String = "",
    compression: Compression = Compression.None,
    balancing: Balancing = Balancing.PickFirst,
): Channel ! Status
```

A channel to a server, which connects with its first call.

```tondo
import std.console
import std.grpc
import std.time

/// A channel round robin over two servers, with each call timed; a client
/// of a `.proto` service takes it: `users.UserServiceClient.on(channel)`.
fn servers(): grpc.Channel ! grpc.Status {
    let balanced = grpc.connect("http://10.0.0.1:50051,10.0.0.2:50051", balancing: grpc.Balancing.RoundRobin)?
    balanced.onCall((call, next) {
        let started = time.now()
        let outcome = next(call)
        console.println("{call.method} took {started.elapsed()}")
        outcome?
    })
}
```

#### reflection

```tondo
pub fn reflection(services: Array[Service]): Service
```

Server reflection, as `grpc.reflection.v1` and `v1alpha` give it, which
tools as grpcurl read to call a server without its `.proto` files:
`app.grpc(grpc.reflection([users, health.service()]))`. It lists these
services and gives the files that declare them.

### Code

```tondo
pub enum Code {
    Ok
    Cancelled
    Unknown
    InvalidArgument
    DeadlineExceeded
    NotFound
    AlreadyExists
    PermissionDenied
    ResourceExhausted
    FailedPrecondition
    Aborted
    OutOfRange
    Unimplemented
    Internal
    Unavailable
    DataLoss
    Unauthenticated
}
```

The code of a call's status, as gRPC numbers them from 0.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    if request.name.isEmpty() {
        fail grpc.Status { code: grpc.Code.InvalidArgument, message: "a name is needed" }
    }
    HelloReply { message: "Hello, {request.name}!" }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }] })
let server = app.start()?
let client = grpc.connect(server.url())?
match client.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest {}) {
    ok(reply) => console.println(reply.message)
    err(status) if status.code == grpc.Code.InvalidArgument => console.println("fix the request ({status.code.number()}): {status.message}")
    err(status) => console.println("{status}")
}
console.println("{grpc.Code.of(5)}")
server.stop()
```

Implements `Display`.

#### Code.number

```tondo
pub fn Code.number(self): Int
```

The code's number.

#### Code.of

```tondo
pub fn Code.of(number: Int): Code
```

The code of a number, `Unknown` for one gRPC does not have.

### Status

```tondo
pub type Status = {
    code: Code
    message: String = ""
    details: Array[protobuf.Any] = []
}
```

How a call failed: its code, a message for people and details for
programs, as `google.rpc.Status` carries them.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    match Int.parse(request.name) {
        ok(_) => fail grpc.Status { code: grpc.Code.InvalidArgument, message: "a name is not a number" }
        err(_) => HelloReply { message: "Hello, {request.name}!" }
    }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }] })
let server = app.start()?
let client = grpc.connect(server.url())?
for name in ["Ada", "42"] {
    match client.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name }) {
        ok(reply) => console.println(reply.message)
        err(status) => console.println("{status.code}: {status.message}")
    }
}
server.stop()
```

Implements `Display`.

#### Status.from

```tondo
pub fn Status.from[E: Display](error: E): Status
```

A failure of the server's own, for `result.mapErr(grpc.Status.from)?`:
`Internal`, with the error's text.

### Call

```tondo
pub type Call = {
    method: String = ""
    metadata: Map[String, String] = [:]
    peer: String = ""
    timeout: time.Duration? = none
}
```

What a call carries besides its messages: the method, as
`/users.v1.UserService/Get`, its metadata, the peer's address and the
time left before its deadline.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    // What the call carries besides its message.
    let call = grpc.call()
    if call.metadata.get("authorization") != some("Bearer secret") {
        fail grpc.Status { code: grpc.Code.Unauthenticated, message: "who are you?" }
    }
    HelloReply { message: "Hello, {request.name}, through {call.method}" }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }] })
let server = app.start()?
let client = grpc.connect(server.url())?
let metadata = ["authorization": "Bearer secret"]
console.println(client.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name: "Ada" }, metadata: metadata)?.message)
server.stop()
```

### Stream

```tondo
pub type Stream[T] = {
    // private fields
}
```

The messages of a stream, each as it arrives: `for each in stream { let
message = each? }`. A failure ends it.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn countdown(request: HelloRequest, replies: Sender[HelloReply]): !grpc.Status {
    for n in [3, 2, 1] {
        replies.send(HelloReply { message: "{request.name} in {n}" })
    }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Launch", methods: [grpc.Method { name: "Countdown", handler: (call) { grpc.serverStream(call, countdown)? } }] })
let server = app.start()?
let replies = grpc.connect(server.url())?.serverStream[HelloRequest, HelloReply]("/demo.Launch/Countdown", HelloRequest { name: "lift-off" })
for reply in replies {
    console.println(reply?.message)
}
server.stop()
```

Implements `Iterator[T ! Status]`.

### Service

```tondo
pub type Service = {
    name: String
    methods: Array[Method]
    /// The `.proto` file that declares it, which reflection gives.
    files: Array[protobuf.FileDescriptor]
    // private fields
}
```

A service a server serves: its full name and its methods, which a
`.proto` file's service gives as `users.userService(value)`.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    HelloReply { message: "Hello, {request.name}!" }
}

// A service written by hand; a `.proto` file under `src` makes one too.
let greeter = grpc.Service {
    name: "demo.Greeter"
    methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }]
}
var app = http.App()
app.grpc(greeter.onCall((call, next) {
    console.println("serving {call.method}")
    next()?
}))
let server = app.start()?
console.println(grpc.connect(server.url())?.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name: "Ada" })?.message)
server.stop()
```

Implements `http.GrpcService`.

#### Service.onCall

```tondo
pub fn Service.onCall(self, interceptor: fn(Call, fn(): !Status): !Status): Service
```

Wraps each call of the service: `interceptor` gets the call and `next`,
which runs the method in the call's task, so it can refuse the call
with `fail`, give the method contexts with `with`, time it or change
its status; a panic of the method reaches it as `Internal`. The first
added is the outermost.

```tondo
import std.diagnostics
import std.grpc
import std.http
import std.log

let failed = diagnostics.counter("health.failed")
let health = grpc.Health()
var app = http.App()
// Every call of the service: logged, and its failures counted.
app.grpc(health.service().onCall((call, next) {
    match next() {
        ok(_) => log.info("served", ["method": call.method])
        err(status) => {
            failed.add()
            log.warn("failed", ["method": call.method, "status": "{status}"])
            fail status
        }
    }
}))
```

#### Service.onMessage

```tondo
pub fn Service.onMessage(self, observer: fn(Call, Direction, reflect.Value)): Service
```

Shows `observer` each message of each call, as it is received or sent.

#### Service.compress

```tondo
pub fn Service.compress(self): Service
```

Compresses the messages the service sends with gzip, to each client
that says it reads gzip. Every server reads compressed messages.

### Method

```tondo
pub type Method = {
    name: String
    handler: fn(ServerCall): !Status
    noSideEffects: Bool = false
}
```

A method of a service: its name, what serves it, and whether it has no
side effects, which lets Connect call it with a `GET`.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    HelloReply { message: "Hello, {request.name}!" }
}

// With no side effects, Connect clients may call it with a `GET`, which caches.
let method = grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? }, noSideEffects: true }
var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [method] })
let server = app.start()?
console.println(grpc.connect(server.url())?.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name: "Ada" })?.message)
server.stop()
```

### Direction

```tondo
pub enum Direction {
    Received
    Sent
}
```

Whether a message was received or sent by the side that sees it.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    HelloReply { message: "Hello, {request.name}!" }
}

let greeter = grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }] }
var app = http.App()
// Each message of each call, as the service receives or sends it.
app.grpc(greeter.onMessage((call, direction, message) {
    let arrow = match direction {
        grpc.Direction.Received => "<-"
        grpc.Direction.Sent => "->"
    }
    console.println("{call.method} {arrow} {message}")
}))
let server = app.start()?
_ = grpc.connect(server.url())?.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name: "Ada" })?
server.stop()
```

Implements `Display`.

### ServerCall

```tondo
pub type ServerCall = {
    // private fields
}
```

A call being served: the messages it receives and sends.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

// A handler that reads and writes the call's messages itself: here, an
// answer to every request until the client stops sending.
fn chat(call: grpc.ServerCall): !grpc.Status {
    for {
        match call.receive[HelloRequest]()? {
            some(request) => call.send(HelloReply { message: "Hi, {request.name}" })?
            none => break
        }
    }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Chat", handler: chat }] })
let server = app.start()?
let talk = grpc.connect(server.url())?.bidiStream[HelloRequest, HelloReply]("/demo.Greeter/Chat")
talk.send(HelloRequest { name: "Ada" })?
talk.send(HelloRequest { name: "Bo" })?
talk.closeSend()
for reply in talk.replies() {
    console.println(reply?.message)
}
server.stop()
```

#### ServerCall.receive

```tondo
pub fn ServerCall.receive[T: protobuf.Message + reflect.Reflect](self): T? ! Status
```

The next message of the call, or `none` after the client's last.

#### ServerCall.send

```tondo
pub fn ServerCall.send[T: protobuf.Message + reflect.Reflect](self, message: T): !Status
```

Sends a message to the client.

### Compression

```tondo
pub enum Compression {
    None
    Gzip
}
```

How a client compresses the messages it sends; every client reads
compressed ones.

```tondo
import std.grpc

// Each message gzipped, worth it for large ones over a slow network.
let client = grpc.connect("https://api.example.com", compression: grpc.Compression.Gzip)?
_ = client
```

### Balancing

```tondo
pub enum Balancing {
    /// All on the first address that answers, in order, until it drops.
    PickFirst
    /// Each on the next of the addresses that answer.
    RoundRobin
}
```

How a channel spreads its calls over the addresses of its target.

```tondo
import std.grpc

// The calls spread over the addresses the name resolves to, again as they change.
let client = grpc.connect("dns:///users.internal:50051", balancing: grpc.Balancing.RoundRobin)?
_ = client
```

### Channel

```tondo
pub type Channel = {
    // private fields
}
```

A connection to a server, which its calls share: `http://host:port`, or
`https://host:port` for TLS, which trusts the system's authorities and,
as the HTTP client does, those of `trust`; `certificate` and `key`
show the server who the client is, and `compression` compresses what it
sends. A target may name several servers, `http://a:50051,b:50051`,
and a host's name may resolve to several addresses: `balancing` says
which one each call goes to.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf
import std.time

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn hello(request: HelloRequest): HelloReply ! grpc.Status {
    HelloReply { message: "Hello, {request.name}!" }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "Hello", handler: (call) { grpc.unary(call, hello)? } }] })
let server = app.start()?
// A channel keeps its connection for every call; its interceptors see each.
let client = grpc.connect(server.url())?.onCall((call, next) {
    let started = time.now()
    let outcome = next(call)
    console.println("{call.method} took {started.elapsed()}")
    outcome?
})
console.println(client.unary[HelloRequest, HelloReply]("/demo.Greeter/Hello", HelloRequest { name: "Ada" })?.message)
server.stop()
```

#### Channel.onCall

```tondo
pub fn Channel.onCall(self, interceptor: fn(Call, fn(Call): !Status): !Status): Channel
```

Wraps each call the channel makes: `interceptor` gets the call and
`next`, which makes it with the metadata and deadline of the `Call` it
is given and gives its status when it ends. A unary call runs the
interceptors in the caller's task and fails as they do; a stream runs
them in a task of their own, with the caller's contexts, while the
caller reads the call `next` made. The first added is the outermost.

#### Channel.onMessage

```tondo
pub fn Channel.onMessage(self, observer: fn(Call, Direction, reflect.Value)): Channel
```

Shows `observer` each message of each call, as it is sent or received.

#### Channel.unary

```tondo
pub fn Channel.unary[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](self, path: String, request: In, metadata: Map[String, String] = [:]): Out ! Status
```

A unary call: one request, one reply.

#### Channel.serverStream

```tondo
pub fn Channel.serverStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](self, path: String, request: In, metadata: Map[String, String] = [:]): Stream[Out]
```

A call that receives a stream: one request, and the replies.

#### Channel.clientStream

```tondo
pub fn Channel.clientStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](self, path: String, metadata: Map[String, String] = [:]): Upload[In, Out]
```

A call that sends a stream of requests and receives one reply.

#### Channel.bidiStream

```tondo
pub fn Channel.bidiStream[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
](self, path: String, metadata: Map[String, String] = [:]): Duplex[In, Out]
```

A call that sends a stream of requests and receives a stream of replies.

### Upload

```tondo
pub type Upload[In, Out] = {
    // private fields
}
```

A call that sends a stream: `send` each request, then `finish` for the
reply.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn greetAll(requests: grpc.Stream[HelloRequest]): HelloReply ! grpc.Status {
    var names: Array[String] = []
    for request in requests {
        names.push(request?.name)
    }
    HelloReply { message: "Hello, {names.join(" and ")}!" }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Greeter", methods: [grpc.Method { name: "GreetAll", handler: (call) { grpc.clientStream(call, greetAll)? } }] })
let server = app.start()?
let upload = grpc.connect(server.url())?.clientStream[HelloRequest, HelloReply]("/demo.Greeter/GreetAll")
for name in ["Ada", "Bo", "Cy"] {
    upload.send(HelloRequest { name })?
}
console.println(upload.finish()?.message)
server.stop()
```

#### Upload

```tondo
pub fn Upload[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
].send(self, message: In): !Status
```

Sends a request; a call that ended fails with its status.

#### Upload

```tondo
pub fn Upload[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
].finish(self): Out ! Status
```

Says the requests are all sent, and gives the reply.

### Duplex

```tondo
pub type Duplex[In, Out] = {
    // private fields
}
```

A call that sends and receives streams: `send` requests, `closeSend`
after the last, and read the replies as a `Stream`.

```tondo
import std.console
import std.grpc
import std.http
import std.protobuf

pub type HelloRequest = {
    @protobuf.Field(1)
    name: String = ""
}

pub type HelloReply = {
    @protobuf.Field(1)
    message: String = ""
}

fn echo(requests: grpc.Stream[HelloRequest], replies: Sender[HelloReply]): !grpc.Status {
    for request in requests {
        replies.send(HelloReply { message: "echo {request?.name}" })
    }
}

var app = http.App()
app.grpc(grpc.Service { name: "demo.Echo", methods: [grpc.Method { name: "Echo", handler: (call) { grpc.bidiStream(call, echo)? } }] })
let server = app.start()?
let talk = grpc.connect(server.url())?.bidiStream[HelloRequest, HelloReply]("/demo.Echo/Echo")
talk.send(HelloRequest { name: "one" })?
talk.send(HelloRequest { name: "two" })?
talk.closeSend()
for reply in talk.replies() {
    console.println(reply?.message)
}
console.println("trailers: {talk.trailers()}")
server.stop()
```

#### Duplex

```tondo
pub fn Duplex[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
].send(self, message: In): !Status
```

Sends a request; a call that ended fails with its status.

#### Duplex.closeSend

```tondo
pub fn Duplex[In, Out].closeSend(self)
```

Says the requests are all sent.

#### Duplex

```tondo
pub fn Duplex[
    In: protobuf.Message + reflect.Reflect,
    Out: protobuf.Message + reflect.Reflect,
].replies(self): Stream[Out]
```

The replies, as they arrive.

#### Duplex.headers

```tondo
pub fn Duplex[In, Out].headers(self): Map[String, String]
```

The metadata the server answered with, once its first message or its
status came.

#### Duplex.trailers

```tondo
pub fn Duplex[In, Out].trailers(self): Map[String, String]
```

The metadata that followed the server's last message, once the replies
ended; the status is not among it.

### ServingStatus

```tondo
pub enum ServingStatus {
    Unknown
    Serving
    NotServing
    /// A service the server does not know, which only a watch gives.
    ServiceUnknown
}
```

Whether a server serves a service, as `grpc.health.v1` tells it.

```tondo
import std.grpc
import std.http
import std.time

let health = grpc.Health()
health.set("demo.Greeter", grpc.ServingStatus.Serving)
var app = http.App()
app.grpc(health.service())
let server = app.start(port: 50051)?
// While it drains for a stop, load balancers stop sending it calls.
health.set("demo.Greeter", grpc.ServingStatus.NotServing)
server.stop(grace: time.seconds(10))
```

Implements `Display`.

### Health

```tondo
pub type Health = {
    // private fields
}
```

The health a server tells of itself and its services, as
`grpc.health.v1` asks it, which load balancers and orchestrators read:
`app.grpc(health.service())`, then `health.set("users.v1.UserService",
grpc.ServingStatus.NotServing)` as it changes. The server, `""`,
starts `Serving`.

```tondo
import std.grpc
import std.http

// The standard health service, which load balancers and Kubernetes ask;
// the empty name is the whole server.
let health = grpc.Health()
health.set("", grpc.ServingStatus.Serving)
var app = http.App()
app.grpc(health.service())
app.grpc(grpc.reflection([health.service()]))
app.listen(port: 50051)?
```

#### Health

```tondo
pub fn Health(): Health
```

Health whose server serves, and no service yet.

#### Health.set

```tondo
pub fn Health.set(self, service: String, status: ServingStatus)
```

Sets a service's status, by its full name, or the server's, `""`;
those who watch it get the new one.

#### Health.service

```tondo
pub fn Health.service(self): Service
```

The service `grpc.health.v1.Health`: `Check` gives a status, or
`NotFound` for a service it does not know, `List` gives them all, and
`Watch` gives one, then each change.
