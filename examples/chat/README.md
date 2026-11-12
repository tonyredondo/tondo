# chat: rooms over WebSockets

A chat with rooms. The server answers a WebSocket for each person, tells a
room who came, who spoke and who left, gives a newcomer what was said
before, and serves the page itself, which is kept inside the executable.
Nothing in it is locked: the rooms belong to one actor, each connection is
another, and they send each other messages. About 300 lines of Tondo; the
WebSockets, the actors, the JSON and the embedded files came with `tondo`.

```text
chat/
  tondo.toml
  src/
    main.to                       listens
    rooms/rooms.to                the hub: an actor that owns the rooms
    rooms/rooms_test.to           the hub, tested by sending it messages
    web/web.to                    the routes, and the actor of each connection
    web/public/                   the page: index.html, app.js, style.css
  tests/chat.to                   two people talking through a real server
```

## Run it

```sh
tondo run examples/chat                 # http://localhost:8080
tondo run examples/chat -- 9000
tondo test examples/chat
```

Open the address in two windows, give two names and the same room, and
talk. What goes over each WebSocket is a line of JSON for each thing that
happens. These are the lines `ada` receives when `leo` comes into her room,
she greets him and he leaves:

```text
{"kind":"welcome","room":"tea","from":"","text":"","people":["ada"]}
{"kind":"joined","room":"tea","from":"leo","text":"","people":["ada","leo"]}
{"kind":"said","room":"tea","from":"ada","text":"hello, leo","people":[]}
{"kind":"left","room":"tea","from":"leo","text":"","people":["ada"]}
```

```text
$ curl localhost:8080/rooms
{"tea":1}
```

## What to look at

**The rooms have one owner.** [`rooms.to`](src/rooms/rooms.to) starts an
actor, the hub: a task that owns everything about the rooms and is told
what happens, one message at a time. Its handler is an ordinary function
that changes its state,

```text
fn handle(state: var State, told: Told) {
    match told {
        Told.Enter(id, room, name, outbox) => { ... }
        Told.Say(id, text) => { ... }
        Told.Leave(id) => { ... }
    }
}
```

and there is no lock in it, since no other task can reach that state. Two
people speaking at once are two messages in the hub's mailbox, handled one
after the other.

**What happens is a type.** `Told` is an enum of the three things a
connection can tell the hub. A connection that comes in gives, in its
message, the address the hub should answer to: an
`executor.ActorRef[String]`. An address can be copied, kept in a map and
sent anywhere, and it does not keep its actor alive.

**Each connection is an actor too.** In [`web.to`](src/web/web.to), the
handler of a WebSocket starts an actor whose state is the socket and whose
handler writes a line to it. The handler's own task then reads what the
browser sends and tells the hub. So a connection is two tasks, one reading
and one writing, that share nothing but addresses:

```text
let mailbox = executor.actor(socket, rooms.Waiting, (to: var http.WebSocket, line: String): !http.HttpError {
    to.sendText(line)?
})?
hub.send(rooms.Told.Enter(id, room, name, mailbox.reference()))?
for message in socket { ... hub.send(rooms.Told.Say(id, text))? ... }
```

**A slow reader costs a bounded amount.** A mailbox holds `Waiting` lines.
The hub sends with `trySend`, which never waits: a line that finds a
mailbox full is counted instead of kept, and its reader is told how many
went by once there is room again. A browser on a bad network holds up its
own actor and nobody else, and the memory a connection can take has a
limit.

**An actor lives while its value is held.** `web.Chat` holds the hub, and
`main` holds the `Chat` while it listens. A connection's mailbox is a local
of its handler: when the handler ends, the mailbox closes, what was already
in it is written, and its task is gone. Nothing is left running that nobody
owns.

**A connection cleans up however it ends.** The handler's `defer` tells the
hub that the person left, whether the browser closed the socket, the
network failed or the server is stopping. It goes through `within`, in a
function of its own: a task that was cancelled does not wait, not even to
send, unless a deadline gives it the time.

**Actors are tested by sending them messages.** In
[`rooms_test.to`](src/rooms/rooms_test.to) a guest is an actor that keeps
what it is sent. A test tells the hub what happened, stops it, which
handles what its mailbox still held, and reads what each guest heard. One
guest reads from behind a gate that the test opens when it chooses, which
is how the test for the reader who cannot keep up decides exactly which
lines find the mailbox full. These tests start tasks, so `tondo test` runs
each one in the orders of them that could change its result.

**What the rest may read.** The hub keeps how many people each room has in
a `sync.Map`, a collection made for several tasks, which `GET /rooms`
reads. It is the one thing the hub shares, and it is only ever written
there.

**Events are a type.** `Event` is a record and `Kind` an enum whose variants
are named for the wire with `@name("joined")`. `json.encode` writes them and
the tests read them back with `json.decode[Event]`, so the page's
`switch (event.kind)` and the server agree by construction.

**The page is in the executable.** `app.files("", embed.files("public"))`
serves the three files of `src/web/public`, which the compiler put inside
the program. The whole chat is one file to copy to a server.

**The tests talk over WebSockets.** [`tests/chat.to`](tests/chat.to) starts
the server on a free port and opens connections with
`http.WebSocket.connect`, as a browser does. Each `told(socket)` waits for
the next event under `within time.seconds(5)`, so a line that never comes
fails the test instead of hanging it.

## Going further

- `tondo run examples/chat --watch` reloads the code as it is saved and
  keeps the program's state: edit what the hub does with a `Say`, save, and
  the people in the room stay in it.
- `tondo build examples/chat --static --output chat` makes the one file.
- Behind TLS, `listen(port: 443, tls: some(http.Tls.files("cert.pem",
  "key.pem")))` and the page's `wss:` are all that changes.
