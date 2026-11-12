# std.sql

One way to talk to SQLite, PostgreSQL and MySQL. A query is a string
literal whose `{values}` are parameters, never text, so a query cannot be
injected into. Rows are read into the program's types by the names of
their columns, and a transaction is a closure that commits when it ends
and rolls back when it fails.

- [Queries and rows](#queries-and-rows)
- [Transactions and migrations](#transactions-and-migrations)
- [What to reach for](#what-to-reach-for)

## Queries and rows

```tondo
import std.console
import std.sql

type Book = { id: Int, title: String, year: Int, read: Bool }

fn main(): !sql.SqlError {
    let db = sql.open("sqlite::memory:")?
    db.execute("CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT NOT NULL, year INTEGER NOT NULL, read INTEGER NOT NULL DEFAULT 0)")?

    let wanted = [("Dune", 1965), ("Emma", 1815), ("Robert'); DROP TABLE books;--", 2007)]
    for (title, year) in wanted {
        // Each `{value}` goes to the database as a parameter.
        db.execute("INSERT INTO books (title, year) VALUES ({title}, {year})")?
    }

    let before = 1970
    let old = db.query[Book]("SELECT id, title, year, read FROM books WHERE year < {before} ORDER BY year")?
    console.println("{old.map((book) { "{book.title} ({book.year})" })}")

    let count = db.queryOne[Int]("SELECT count(*) FROM books")?
    let missing = db.queryOne[Book?]("SELECT id, title, year, read FROM books WHERE id = {99}")?
    console.println("{count} books, and number 99 is {missing.isNone()}")
}
```

```text output
[Emma (1815), Dune (1965)]
3 books, and number 99 is true
```

- A query is a [`Query`](#query), which a string literal becomes where one
  is expected. A `{value}` in it is bound, with its type, and a string
  built some other way is not a query: text reaches SQL only through
  [`raw`](#raw) and [`identifier`](#identifier), where it is meant.
- [`Database.query`](#databasequery) gives an array of a record, a tuple or
  a single value, [`Database.queryOne`](#databasequeryone) one row, or a
  `T?` when there may be none, and
  [`Database.execute`](#databaseexecute) how many rows changed.
- A column is matched to a field by name, and a value that does not fit the
  field's type is an error, not a zero.
- `sql.open` takes `sqlite:path`, `postgres://user:secret@host/name` or
  `mysql://...`, and the same queries run on each.

## Transactions and migrations

```tondo
import std.console
import std.sql

type Account = { name: String, balance: Int }

fn move(db: sql.Database, from: String, to: String, amount: Int): !sql.SqlError {
    db.transaction((tx): Unit ! sql.SqlError {
        tx.execute("UPDATE accounts SET balance = balance - {amount} WHERE name = {from}")?
        tx.execute("UPDATE accounts SET balance = balance + {amount} WHERE name = {to}")?
        let left = tx.queryOne[Int]("SELECT balance FROM accounts WHERE name = {from}")?
        if left < 0 {
            // Failing rolls both updates back.
            fail sql.SqlError.of(sql.ErrorKind.Check, "{from} has not got {amount}")
        }
    })
}

fn main(): !sql.SqlError {
    let db = sql.open("sqlite::memory:")?
    db.execute("CREATE TABLE accounts (name TEXT PRIMARY KEY, balance INTEGER NOT NULL)")?
    db.execute("INSERT INTO accounts VALUES ('ada', 100), ('leo', 20)")?

    move(db, "ada", "leo", 30)?
    console.println("{move(db, "leo", "ada", 500).isErr()}")
    let accounts = db.query[Account]("SELECT name, balance FROM accounts ORDER BY name")?
    console.println("{accounts.map((account) { "{account.name} {account.balance}" })}")
}
```

```text output
true
[ada 70, leo 50]
```

[`Database.migrate`](#databasemigrate) applies a directory of `.sql` files,
each once and in order, and remembers which it applied. With
`embed.files("migrations")` the files are inside the executable, so the
schema travels with the program.

## What to reach for

| To | Use |
| --- | --- |
| Open a database | [`open`](#open) |
| Change rows | [`Database.execute`](#databaseexecute) |
| Read rows into a type | [`Database.query`](#databasequery), [`Database.queryOne`](#databasequeryone) |
| Do several things or none | [`Database.transaction`](#databasetransaction) |
| Keep the schema up to date | [`Database.migrate`](#databasemigrate) |
| Put a name or a piece of SQL in a query, on purpose | [`identifier`](#identifier), [`raw`](#raw), [`join`](#join) |
| Pass a list as one parameter | [`array`](#array) |

The [`todo-api`](../../examples/todo-api) example keeps its tasks in SQLite,
with migrations in the executable and tests on a database in memory.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.sql` shows the same in a terminal.

- [Functions](#functions): [`raw`](#raw), [`identifier`](#identifier), [`join`](#join), [`array`](#array), [`open`](#open)
- [ErrorKind](#errorkind)
- [SqlError](#sqlerror): [`SqlError.of`](#sqlerrorof)
- [Value](#value)
- [Query](#query)
- [Operation](#operation)
- [Statement](#statement)
- [Outcome](#outcome)
- [Done](#done)
- [Connection](#connection)
- [Driver](#driver)
- [Database](#database): [`Database.use`](#databaseuse), [`Database.execute`](#databaseexecute), [`Database.query`](#databasequery), [`Database.queryOne`](#databasequeryone), [`Database.transaction`](#databasetransaction), [`Database.migrate`](#databasemigrate), [`Database.close`](#databaseclose)

### Functions

#### raw

```tondo
pub fn raw(text: String): Query
```

SQL made while the program runs, as it is: it holds no parameters, and
whatever made the text decides what it does. Values go in a literal.

#### identifier

```tondo
pub fn identifier(name: String): Query
```

A name of a table or a column, quoted as the database quotes names, for
one chosen while the program runs: `"SELECT * FROM {sql.identifier(table)}"`.

#### join

```tondo
pub fn join(parts: Array[Query], separator: String = ", "): Query
```

The queries one after another with `separator` between them:
`sql.join(conditions, " AND ")`.

#### array

```tondo
pub fn array[T: serialization.Encode](values: Array[T]): Query
```

The array as one parameter, for a database with arrays:
`"WHERE id = ANY({sql.array(ids)})"` in PostgreSQL.

#### open

```tondo
pub fn open(url: String, connections: Int = 10, driver: Driver? = none): Database ! SqlError
```

The database of a URL, with up to `connections` at once:
`sqlite:app.db`, `sqlite::memory:`, `postgres://user:secret@host/name`
or `mysql://user:secret@host/name`, or another for `driver`. One
connection opens now, so a wrong URL fails here.

### ErrorKind

```tondo
pub enum ErrorKind {
    /// It could not connect, or lost the connection.
    Connection
    /// `queryOne` found no row, for a type that is not optional.
    NoRows
    /// A unique key would repeat.
    Unique
    /// A foreign key names a row that does not exist, or is still named.
    ForeignKey
    /// A column that cannot be null would be.
    NotNull
    /// A check constraint would not hold.
    Check
    /// The transaction met another and has to be tried again.
    Conflict
    /// The SQL does not read.
    Syntax
    /// The statement took longer than allowed.
    Timeout
    /// A row could not be read into the type.
    Decode
    /// Anything else the database says.
    Other
}
```

What kind of failure a database met.

```tondo
import std.console
import std.sql

let db = sql.open("sqlite::memory:")?
_ = db.execute("CREATE TABLE users (email TEXT PRIMARY KEY, name TEXT NOT NULL)")?
for email in ["ada@example.com", "ada@example.com"] {
    match db.execute("INSERT INTO users (email, name) VALUES ({email}, {"Ada"})") {
        ok(_) => console.println("added {email}")
        err(error) if error.kind == sql.ErrorKind.Unique => console.println("{email} is taken")
        err(error) => console.println("{error}")
    }
}
```

Implements `Display`.

### SqlError

```tondo
pub type SqlError = {
    kind: ErrorKind
    message: String
    code: String?
    constraint: String?
}
```

A failure of a database: its kind, its message, the database's code
(PostgreSQL's SQLSTATE, MySQL's number, SQLite's extended code) and the
constraint or column it names.

```tondo
import std.console
import std.sql

let db = sql.open("sqlite::memory:")?
_ = db.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, price INTEGER CHECK (price > 0))")?
match db.execute("INSERT INTO items (price) VALUES ({-5})") {
    ok(_) => console.println("added")
    err(error) => console.println("{error.kind}: {error.message} (code {error.code}, constraint {error.constraint})")
}
```

Implements `Display`, `reflect.Reflect`.

#### SqlError.of

```tondo
pub fn SqlError.of(kind: ErrorKind, message: String): SqlError
```

An error of a kind and message.

### Value

```tondo
pub enum Value {
    Null
    Bool(Bool)
    Int(Int)
    Float(Float)
    /// An exact number, as its digits: a `math.Decimal`, a `NUMERIC`.
    Number(String)
    Text(String)
    Bytes(Array[Byte])
    DateTime(time.DateTime)
    /// An array as one parameter, which `sql.array` makes.
    List(Array[Value])
}
```

A value of a database: a parameter or a column of a row.

```tondo
import std.console
import std.sql

var db = sql.open("sqlite::memory:")?
// The parameters of each statement, as the driver receives them.
db.use((statement, next) {
    let shown = statement.parameters.map((parameter) {
        match parameter {
            sql.Value.Null => "NULL"
            sql.Value.Int(number) => "int {number}"
            sql.Value.Text(text) => "text {text}"
            _ => "other"
        }
    })
    console.println("{shown}")
    let outcome = next(statement)
    outcome?
})
let name: String? = none
_ = db.queryOne[Int]("SELECT {42} + length({"tea"}) + coalesce({name}, 0)")?
```

Implements `Display`.

### Query

```tondo
pub type Query = {
    // private fields
}
```

A statement of SQL, written as a string literal where one is expected:
each `{value}` it interpolates is a parameter, never text, so it cannot
be injected; a `{query}` is spliced in, and an `{array}` is the list
`($1, $2, …)` of its elements, for `IN`.

```tondo
import std.console
import std.sql

pub type User = { id: Int, name: String }

let db = sql.open("sqlite::memory:")?
_ = db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)")?
for name in ["Ada", "Bo", "Cy"] {
    _ = db.execute("INSERT INTO users (name) VALUES ({name})")?
}
// Pieces made apart and joined, each `{value}` still a parameter.
let table = sql.identifier("users")
var conditions: Array[sql.Query] = ["id > {0}"]
conditions.push("name IN {["Ada", "Cy"]}")
let order = sql.raw("ORDER BY name DESC")
for user in db.query[User]("SELECT id, name FROM {table} WHERE {sql.join(conditions, " AND ")} {order}")? {
    console.println("{user.id} {user.name}")
}
```

### Operation

```tondo
pub enum Operation {
    Query
    Execute
    Begin
    Commit
    Rollback
}
```

What a statement does: reads, writes, or begins, commits or rolls back a
transaction.

```tondo
import std.console
import std.sql

var db = sql.open("sqlite::memory:")?
db.use((statement, next) {
    let what = match statement.operation {
        sql.Operation.Query => "query"
        sql.Operation.Execute => "execute"
        sql.Operation.Begin => "begin"
        sql.Operation.Commit => "commit"
        sql.Operation.Rollback => "rollback"
    }
    console.println(what)
    let outcome = next(statement)
    outcome?
})
db.transaction((tx): Unit ! sql.SqlError {
    _ = tx.execute("CREATE TABLE notes (text TEXT)")?
    _ = tx.query[String]("SELECT text FROM notes")?
})?
```

Implements `Display`.

### Statement

```tondo
pub type Statement = {
    text: String
    parameters: Array[Value]
    operation: Operation
    system: String
    database: String
    host: String?
}
```

A statement as the driver runs it, which `use` sees and may change, as
to add a comment with the trace's context: its text with the driver's
placeholders, its parameters, what it does, and the database's system as
OpenTelemetry names it (`postgresql`, `mysql`, `sqlite`), name and host.

```tondo
import std.console
import std.sql
import std.time

var db = sql.open("sqlite::memory:")?
// Each statement, timed, with where it went.
db.use((statement, next) {
    let started = time.now()
    let outcome = next(statement)
    console.println("{statement.system} {statement.database}: {statement.text} {statement.parameters.length()} parameters in {started.elapsed()}")
    outcome?
})
_ = db.queryOne[Int]("SELECT {1} + {2}")?
```

### Outcome

```tondo
pub type Outcome = {
    columns: Array[String]
    rows: Array[Array[Value]]
    affected: Int
    lastId: Int?
}
```

What a statement gave: the names of its columns and its rows, or the
rows it changed and the id of the last one it added.

```tondo
import std.console
import std.sql

var db = sql.open("sqlite::memory:")?
// What a statement gave, as a middleware sees it before the rows are read.
db.use((statement, next) {
    let outcome = next(statement)?
    console.println("{outcome.columns} {outcome.rows.length()} rows, {outcome.affected} changed")
    outcome
})
_ = db.query[Int]("SELECT 1 AS one UNION SELECT 2")?
```

### Done

```tondo
pub type Done = {
    affected: Int
    lastId: Int?
}
```

What `execute` did: how many rows it changed, and the id of the last one
it added, where the database tells it.

```tondo
import std.console
import std.sql

let db = sql.open("sqlite::memory:")?
_ = db.execute("CREATE TABLE tasks (id INTEGER PRIMARY KEY, done INTEGER DEFAULT 0)")?
let added = db.execute("INSERT INTO tasks (done) VALUES ({0}), ({0})")?
console.println("{added.affected} added, last id {added.lastId}")
let finished = db.execute("UPDATE tasks SET done = 1 WHERE id = {1}")?
console.println("{finished.affected} finished")
```

### Connection

```tondo
pub type Connection = {
    run: fn(Statement): Outcome ! SqlError
    close: fn()
}
```

A connection a driver opened: what runs a statement on it, and what ends
it.

```tondo
import std.console
import std.sql

/// A connection that answers every statement with its own text, as a
/// test double of a database does.
fn echoing(_: String): sql.Connection ! sql.SqlError {
    sql.Connection {
        run: (statement) {
            sql.Outcome { columns: ["text"], rows: [[sql.Value.Text(statement.text)]], affected: 0, lastId: none }
        }
        close: () {}
    }
}

let driver = sql.Driver {
    system: "echo"
    placeholder: (index) { "${index}" }
    identifier: (name) { "\"{name}\"" }
    connect: echoing
    place: (_) { ("echo", none) }
}
let db = sql.open("echo:", driver: some(driver))?
console.println(db.queryOne[String]("SELECT * FROM {sql.identifier("users")} WHERE id = {7}")?)
```

### Driver

```tondo
pub type Driver = {
    system: String
    placeholder: fn(Int): String
    identifier: fn(String): String
    connect: fn(String): Connection ! SqlError
    /// The database's name and host in a URL, for `Statement`.
    place: fn(String): (String, String?)
}
```

What speaks to a kind of database, as a package gives one for another:
`sql.open(url, driver: oracle.driver())`. `connect` opens a connection
to a URL; `placeholder` writes the parameter at a position from 1, as
`$1` or `?`; `identifier` quotes a name; `system` names the database for
`use`.

```tondo
import std.sql

/// A driver for another database, as a package gives one: how it writes
/// parameters and names, how it connects, and where a URL points.
fn oracle(): sql.Driver {
    sql.Driver {
        system: "oracle"
        placeholder: (index) { ":{index}" }
        identifier: (name) { "\"{name.replace("\"", "\"\"")}\"" }
        connect: (url) { fail sql.SqlError.of(sql.ErrorKind.Connection, "no Oracle client at {url}") }
        place: (url) { (url.split("/").last() ?? "", none) }
    }
}

let db = sql.open("oracle://scott@db.example.com/orders", driver: some(oracle()))?
db.close()
```

### Database

```tondo
pub type Database = {
    // private fields
}
```

A database, through a pool of connections that its copies share: tasks
run statements on it at once, each on a connection of its own.

```tondo
import std.console
import std.sql

pub type Account = { id: Int, owner: String, balance: Int }

let db = sql.open("sqlite::memory:", connections: 4)?
_ = db.execute("CREATE TABLE accounts (id INTEGER PRIMARY KEY, owner TEXT, balance INTEGER)")?
_ = db.execute("INSERT INTO accounts (owner, balance) VALUES ({"Ada"}, {100}), ({"Bo"}, {0})")?
// Both changes or neither.
db.transaction((tx): Unit ! sql.SqlError {
    _ = tx.execute("UPDATE accounts SET balance = balance - {30} WHERE owner = {"Ada"}")?
    _ = tx.execute("UPDATE accounts SET balance = balance + {30} WHERE owner = {"Bo"}")?
})?
for account in db.query[Account]("SELECT id, owner, balance FROM accounts ORDER BY id")? {
    console.println("{account.owner}: {account.balance}")
}
let missing = db.queryOne[Account?]("SELECT id, owner, balance FROM accounts WHERE owner = {"Cy"}")?
console.println("{missing.isSome()}")
db.close()
```

Implements `reflect.Reflect`.

#### Database.use

```tondo
pub fn Database.use(
    mut self,
    middleware: fn(Statement, fn(Statement): Outcome ! SqlError): Outcome ! SqlError,
)
```

Wraps each statement, as `App.use` wraps each request: `middleware`
gets the statement and `next`, which runs it, so it can time it, trace
it, change it or refuse it. The first added is the outermost.

#### Database.execute

```tondo
pub fn Database.execute(self, query: Query): Done ! SqlError
```

Runs a statement that writes, and says what it changed.

#### Database.query

```tondo
pub fn Database.query[T: serialization.Decode](self, query: Query): Array[T] ! SqlError
```

The rows of a statement, each read into a `T`: a record by the names of
its columns, as `created_at` for `createdAt`; a value of the only
column; or an option of them.

#### Database.queryOne

```tondo
pub fn Database.queryOne[T: serialization.Decode](self, query: Query): T ! SqlError
```

The first row of a statement as a `T`; without rows, `none` for an
optional `T`, and `NoRows` for another.

#### Database.transaction

```tondo
pub fn Database.transaction[T: ~Copy](self, work: fn(Database): T ! SqlError): T ! SqlError
```

Runs `work` in a transaction on one connection, with a `Database` that
runs its statements there, so a function that takes a `Database` works
in it too. It commits when `work` succeeds and rolls back when it fails
or panics; one inside another is a savepoint.

```tondo
import std.sql

fn transfer(db: sql.Database, from: Int, to: Int, amount: Int): !sql.SqlError {
    db.transaction((tx): Unit ! sql.SqlError {
        _ = tx.execute("UPDATE accounts SET balance = balance - {amount} WHERE id = {from}")?
        _ = tx.execute("UPDATE accounts SET balance = balance + {amount} WHERE id = {to}")?
    })?
}
```

#### Database.migrate

```tondo
pub fn Database.migrate(self, files: fs.FileSystem): !SqlError
```

Applies the `.sql` files of a tree in the order of their names, each once,
whole and in a transaction, keeping which in the table `tondo_migrations`
with a digest of each: a file applied before that changed is an error.

#### Database.close

```tondo
pub fn Database.close(self)
```

Closes the connections the pool holds.
