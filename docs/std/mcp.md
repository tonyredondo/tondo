# std.mcp

the Model Context Protocol served from the methods of a value: each `pub` method is a tool, or a resource or a prompt as its annotations say; over standard input and output, or a route of `std.http`. It speaks the versions `tondo mcp` speaks: each request of 2026-07-28 names its version, and the `initialize` handshake of 2025-11-25 and before.

```tondo
import std.mcp

/// Small arithmetic, as tools of the Model Context Protocol.
pub type Calculator = {}

pub type Sum = {
    /// The numbers to add.
    numbers: Array[Int]
}

/// Adds numbers.
pub fn Calculator.add(self, input: Sum): Int {
    input.numbers.reduce(0, (total, n) { total + n })
}

mcp.serve(Calculator {})?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.mcp` shows the same in a terminal.

- [Functions](#functions): [`serve`](#serve), [`handler`](#handler)
- [Resource](#resource)
- [Prompt](#prompt)
- [McpError](#mcperror)

### Functions

#### serve

```tondo
pub fn serve[T: reflect.Reflect](
    server: T,
    name: String = "",
    version: String = "0.1.0",
): !McpError
```

Serves the methods of `server` over standard input and output, a
message a line, until the input ends; meanwhile the console's output
goes to standard error, since standard output is the protocol's. Each
request is answered in a task of its own.

#### handler

```tondo
pub fn handler[T: reflect.Reflect](
    server: T,
    name: String = "",
    version: String = "0.1.0",
): fn(http.Request): http.Reply ! http.Problem
```

Serves the methods of `server` on a route of `std.http`: a message a
`POST`, answered with JSON, without sessions.

### Resource

```tondo
pub annotation Resource = {
    uri: String
    mime: String = "text/plain"
}
```

Makes a method without parameters, or with the `String` parameters its
URI names in braces, a resource: `@mcp.Resource(uri: "docs://{name}")`.

Implements `reflect.Reflect`.

### Prompt

```tondo
pub annotation Prompt = {}
```

Makes a method a prompt: its `String` parameters are the prompt's
arguments, and the text it returns the user's message.

Implements `reflect.Reflect`.

### McpError

```tondo
pub type McpError = {
    message: String
}
```

Why serving stopped: the protocol's stream failed.

```tondo
import std.console
import std.mcp

pub type Tools = {}

/// Says hello.
pub fn Tools.hello(self): String {
    "hello"
}

match mcp.serve(Tools {}) {
    ok(_) => {}
    err(error) => console.eprintln("the server stopped: {error.message}")
}
```

Implements `Display`, `reflect.Reflect`.
