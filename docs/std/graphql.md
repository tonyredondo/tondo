# std.graphql

GraphQL servers made from the program's types: the fields and `pub` methods of the query's type, and of each type they give, are the schema's fields, and a method's parameters its arguments; the mutation's methods are its mutations. Queries are parsed and run in Tondo over reflection, introspection included, so the tools of GraphQL read the schema.

```tondo
import std.graphql
import std.http

pub type Book = { id: Int, title: String }

/// The API: each `pub` method is a field of the query.
pub type Library = { books: Array[Book] }

/// The books whose title has the text.
pub fn Library.search(self, text: String): Array[Book] {
    self.books.filter((book) { book.title.contains(text) })
}

let schema = graphql.schema(Library { books: [Book { id: 1, title: "Tondo" }] })
var app = http.App()
app.post("/graphql", schema.handler())
app.get("/graphql", schema.handler())
app.listen(port: 8080)?
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.graphql` shows the same in a terminal.

- [Functions](#functions): [`schema`](#schema)
- [Schema](#schema-1): [`Schema.mutation`](#schemamutation), [`Schema.sdl`](#schemasdl), [`Schema.execute`](#schemaexecute), [`Schema.handler`](#schemahandler)

### Functions

#### schema

```tondo
pub fn schema[Q: reflect.Reflect](query: Q): Schema
```

The schema of `query`, whose fields and methods answer queries.

### Schema

```tondo
pub type Schema = {
    // private fields
}
```

A GraphQL schema made from the types of its roots.

```tondo
import std.console
import std.graphql

pub type Book = { id: Int, title: String }

pub type Library = { books: Array[Book] }

pub fn Library.search(self, text: String): Array[Book] {
    self.books.filter((book) { book.title.contains(text) })
}

let schema = graphql.schema(Library { books: [Book { id: 1, title: "Tondo" }, Book { id: 2, title: "Go" }] })
let result = schema.execute(r"""{ search(text: "Ton") { id title } }""")
console.println("{result}")
console.print(schema.sdl())
```

#### Schema.mutation

```tondo
pub fn Schema.mutation[M: reflect.Reflect](self, root: M): Schema
```

The schema with the methods of `root` as its mutations.

#### Schema.sdl

```tondo
pub fn Schema.sdl(self): String
```

The schema in the language of GraphQL's schemas.

#### Schema.execute

```tondo
pub fn Schema.execute(
    self,
    document: String,
    variables: Map[String, json.Value] = [:],
    operation: String? = none,
): json.Value
```

Runs a document: the operation named `operation`, or its only one, with
`variables`; gives `{"data": ..., "errors": [...]}` as GraphQL does.

#### Schema.handler

```tondo
pub fn Schema.handler(self): fn(http.Request): http.Reply ! http.Problem
```

Serves the schema on a route of `std.http`: a `POST` of JSON with
`query`, `variables` and `operationName`, or a `GET` with them in the
query string; a `GET` from a browser without a query gets GraphiQL.
