# chat: rooms over WebSockets

A chat with rooms. The server answers a WebSocket for each person, tells a
room who came, who spoke and who left, gives a newcomer what was said
before, and serves the page itself, which is kept inside the executable.
About 260 lines of Tondo; the WebSockets, the JSON and the embedded files
came with `tondo`.

```text
chat/
  tondo.toml
  src/
    main.to                       listens
    rooms/rooms.to                the rooms, as a value and the functions that change it
    rooms/rooms_test.to           tested with no server, no socket and no task
    web/web.to                    the routes, and the two tasks of each connection
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

**The rooms are a value.** `Rooms` in [`rooms.to`](src/rooms/rooms.to) is a
record of two maps, and `enter`, `say`, `leave` and `take` are functions
that change it. Nothing in the module waits, locks, or knows what a socket
is. So [`rooms_test.to`](src/rooms/rooms_test.to) tests everything the chat
does, from the history a newcomer hears to the reader who cannot keep up,
by calling functions on a `var rooms` and looking at what they left:

```text
var rooms = Rooms {}
rooms.enter(1, "tea", "ada")
rooms.enter(2, "tea", "leo")
rooms.say(1, "hello")
assert(heard(var rooms, 2)? == ["welcome to tea with [ada, leo]", "ada: hello"])
```

**One lock, around the value.** [`web.to`](src/web/web.to) keeps the rooms
in a `sync.Mutex[Rooms]`, the only thing the connections share. Every
change is `all.update((now) { now.say(id, text) })`: the closure has the
value to itself while it runs, and there is no way to reach the rooms
without it.

**Two tasks for each connection.** One reads what the browser sends, in the
handler itself. The other writes what the room says:

```text
for {
    let lines = all.updateWhen((now) { now.hasLinesFor(id) }, (now) { now.take(id) })
    for text in lines {
        socket.sendText(text)?
    }
}
```

`updateWhen` waits until the condition holds and then makes the change, as
one step. The writer sleeps until its connection has lines, with no polling
and no channel to keep in step with the map of members.

**A slow reader costs a bounded amount.** A browser on a bad network holds
up its own writer and nobody else: lines wait for it in `Rooms`, up to
`Waiting` of them, and after that they are counted instead of kept. When it
catches up, it is told how many went by. The room is never held up, and the
memory a connection can take has a limit.

**A connection cleans up however it ends.** The handler's `defer` cancels
the writer and takes the person out of the room, whether the browser closed
the socket, the network failed or the server is stopping. Leaving goes
through `within`, in a function of its own: a task that was cancelled does
not wait, not even for a lock, unless a deadline gives it the time. The
compiler warns about a lock taken in a `defer` for that reason (`W1017`).

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
  keeps the program's state: edit what `say` does, save, and the people in
  the room stay in it.
- `tondo build examples/chat --static --output chat` makes the one file.
- Behind TLS, `listen(port: 443, tls: some(http.Tls.files("cert.pem",
  "key.pem")))` and the page's `wss:` are all that changes.
