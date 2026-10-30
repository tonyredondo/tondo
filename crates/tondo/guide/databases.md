# Databases

`std.sql` speaks to SQLite, PostgreSQL and MySQL through one API. A query
is a string literal where a `sql.Query` is expected: each `{value}` in it is
a parameter, never text, so it cannot be injected. Rows read into the
program's types.

```tondo
import std.console
import std.fs
import std.sql

pub type User = { id: Int, name: String, email: String?, createdAt: String }

fn main(): !sql.SqlError {
    var db = sql.open("sqlite:app.db")?
    db.migrate(fs.disk("migrations"))?
    let name = "Ada"
    _ = db.execute("INSERT INTO users (name, email) VALUES ({name}, {"ada@example.com"})")?
    let ids = [1, 2, 3]
    for user in db.query[User]("SELECT * FROM users WHERE id IN {ids} ORDER BY id")? {
        console.println("{user.id} {user.name}")
    }
    let count = db.queryOne[Int]("SELECT count(*) FROM users")?
    let found = db.queryOne[User?]("SELECT * FROM users WHERE name = {name}")?
    console.println("{count} {found.isSome()}")
}
```

- `sql.open(url, connections: 10)` opens a pool that tasks share:
  `sqlite:app.db`, `sqlite::memory:`, `postgres://user:secret@host/name`,
  `mysql://user:secret@host/name`. One connection opens at once, so a
  wrong URL fails there.
- `execute` gives a `sql.Done` (`affected`, `lastId`); `query[T]` the rows;
  `queryOne[T]` the first, `none` without one for a `T?`, else
  `ErrorKind.NoRows`. A record reads its fields from columns of the same
  name, without case or `_` (`created_at` is `createdAt`); a JSON column
  reads into a record or a list; one column reads as a value.
- `{array}` is the list `(?, ?, ?)` of its elements, for `IN`; `{query}`
  splices a `sql.Query` in, so `let wanted: sql.Query = "id > {1}"` builds
  a condition. `sql.identifier(table)` quotes a name; `sql.raw(text)` is
  SQL made at run time, with no parameters. A `String` is not a query.
- Errors have a kind: `Unique`, `ForeignKey`, `NotNull`, `Check`,
  `Conflict`, `Syntax`, `Timeout`, `Connection`, `NoRows`, `Decode`, with
  the database's `code` and the `constraint` it names.

```tondo
import std.sql

fn transfer(db: sql.Database, from: Int, to: Int, amount: Int): !sql.SqlError {
    db.transaction((tx): Unit ! sql.SqlError {
        _ = tx.execute("UPDATE accounts SET balance = balance - {amount} WHERE id = {from}")?
        _ = tx.execute("UPDATE accounts SET balance = balance + {amount} WHERE id = {to}")?
    })?
}
```

- `transaction` commits when the closure succeeds and rolls back when it
  fails or panics. Its `tx` is a `Database` on the transaction's
  connection, so functions that take a `Database` work inside; one inside
  another is a savepoint.
- `migrate(files)` applies the `.sql` files of a tree in the order of
  their names, each once, whole and in a transaction, and fails if one
  applied before changed: add a new file instead. `embed.files("migrations")`
  keeps them in the executable.
- `db.use((statement, next) { ... })` wraps every statement, as
  `App.use` wraps requests: time it, trace it, log it, change it. It ends
  with `outcome?`:

```tondo
import std.log
import std.sql
import std.time

fn traced(db: sql.Database): sql.Database {
    let logger = log.current()
    var wrapped = db
    wrapped.use((statement, next) {
        let started = time.now()
        let outcome = next(statement)
        let took = "{started.elapsed().milliseconds()}"
        logger.info("sql", ["system": statement.system, "text": statement.text, "ms": took])
        outcome?
    })
    wrapped
}
```

- TLS: `?sslmode=require` (`ssl-mode=required` in MySQL) insists on it;
  the default uses it when the server offers it, except on this machine;
  `disable` never. TLS always verifies the server: give its authority as
  `sslrootcert=path` (`ssl-ca`). PostgreSQL authenticates with SCRAM, MD5
  or a password; MySQL with `caching_sha2_password` or
  `mysql_native_password`.
- A driver for another database is a `sql.Driver`: `sql.open(url, driver:
  oracle.driver())`.
