# notes: an MCP server

A directory of Markdown notes served to an assistant over the Model Context
Protocol: tools to write, search, list and remove them, each note as a
resource, and a prompt. The server is a type whose `pub` methods are the
tools: their documentation is what the model reads, and their parameters
are the arguments, checked before the method runs. About 290 lines of
Tondo; the protocol, the schemas and the validation came with `tondo`.

```text
notes/
  tondo.toml
  src/
    main.to                       the command line: over standard input, or HTTP
    tools/tools.to                what the assistant sees: tools, a resource, a prompt
    library/library.to            the notes: files, names and the search
    library/library_test.to       tested on a file system in memory
  tests/protocol.to               the protocol itself, request by request
```

## Run it

```sh
tondo test examples/notes
tondo build examples/notes --output notes-mcp
```

An MCP client starts the server and talks to it through its input and
output. Its configuration names the program and the directory:

```json
{ "mcpServers": { "notes": { "command": "/path/to/notes-mcp", "args": ["/path/to/notes"] } } }
```

By hand, the protocol is a line of JSON for each request:

```text
$ echo '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"write","arguments":{"name":"green-tea","text":"Buy sencha.\nBrew it at 70 degrees.","tags":["tea"]}}}' | notes-mcp notes
{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"wrote notes://green-tea"}],"isError":false}}

$ cat notes/green-tea.md
tags: tea

Buy sencha.
Brew it at 70 degrees.

$ echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search","arguments":{"query":"sencha"}}}' | notes-mcp notes
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"[{\"name\":\"green-tea\",\"score\":1,\"line\":\"Buy sencha.\"}]"}],"structuredContent":{"result":[{"name":"green-tea","score":1,"line":"Buy sencha."}]},"isError":false}}
```

What the model is told of a tool comes from the code. This is `write` in
the answer to `tools/list`:

```json
{
  "name": "write",
  "description": "Writes a note, replacing the one of that name.",
  "inputSchema": {
    "type": "object",
    "properties": {
      "name": { "description": "The note's name: lower-case letters, digits and `-`, as `green-tea`.", "type": "string" },
      "text": { "description": "Its text, in Markdown.", "type": "string" },
      "tags": { "description": "Words to find it by.", "type": "array", "items": { "type": "string" } }
    },
    "required": ["name", "text"]
  }
}
```

`notes-mcp notes --http 8080` serves the same over HTTP, a request a `POST`
to `/mcp`.

## What to look at

**A tool is a method.** In [`tools.to`](src/tools/tools.to),

```text
/// Writes a note, replacing the one of that name.
pub fn Notes.write(self, input: WriteInput): String ! library.NoteError
```

is the whole declaration of a tool. Its `///` is the description, the
fields of `WriteInput` are its arguments with their own documentation, and a
field with a default is not required. There is no schema to write and none
to keep in step with the code: the schema above is this type. A method that
is not `pub` is no tool, and a `pub` one that cannot be a tool, as one that
gives a channel, is a compile error that says why (`E1912`): a tool is
never left out in silence.

**Arguments are checked before the method runs.** `name` carries
`@validate.Matches("[a-z0-9-]+")` and `limit` a `Min` and a `Max`. A call
that breaks a rule is answered with what is wrong, in words the model can
act on, and the method never sees it:

```text
the arguments of `write` break their rules: name: must match `[a-z0-9-]+`
```

What a model sends is input from outside, and `../etc/passwd` is a name it
may well send. The library checks the name again on its own, since it does
not know who calls it.

**A failure is an answer.** A tool that returns `T ! E` and fails gives the
model the error's `Display`, marked as an error, and the server goes on. So
does a panic. ``there is no note `coffee` `` is something the model can
read and correct.

**Results are data and text.** `search` returns an `Array[library.Match]`.
The client gets it as text, for the model, and as `structuredContent` with
its schema, for a program.

**Resources and prompts are methods with an annotation.**
`@mcp.Resource(uri: r"notes://{name}", mime: "text/markdown")` makes a
method a family of resources, with the part of the address in braces as its
parameter, and `@mcp.Prompt` makes one a prompt whose parameters are its
arguments.

**Two transports, one server.** `mcp.serve(server)` speaks over standard
input and output, and `mcp.handler(server)` is a handler for a route of
`std.http`. [`tests/protocol.to`](tests/protocol.to) uses the second to test
the protocol itself: what the server offers, each tool, the errors, and that
arguments that break their rules write no file.

**The server holds a file system, not a path.** `Notes` has one field, an
`fs.FileSystem`. `main` gives it `fs.disk(directory)`, and the tests of the
protocol give it `fs.memory()`, so the whole server, from the JSON of a
request to the file a tool writes, is tested without touching the disk.

**The notes do not know about the protocol.**
[`library.to`](src/library/library.to) takes an `fs.FileSystem` and knows
nothing of MCP. Its tests run on `fs.memory()`. A command line or a web
page over the same notes would use it as it is.

## Going further

- While it serves over standard input, what the program prints with
  `console` goes to standard error, so a log line never corrupts the
  protocol.
- `tondo mcp` is the same idea at full size: the whole toolchain as tools,
  which `tondo setup` registers with the agents installed.
- The search here reads every note for each query. For many notes, an
  index kept in SQLite with `std.sql`, which the `todo-api` example uses, is
  the next step.
