# Result, Failure and Panic

`T ! E` is `Result[T, E]`: `ok(value)` or `err(error)`. `Failure` says what the program was doing and keeps the cause, and `Panic` is what a `recover` gives of a bug.

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api Result`, `tondo api Failure`, `tondo api Panic` show the same in a terminal.

- [Result](#result): [`Result.isOk`](#resultisok), [`Result.isErr`](#resultiserr), [`Result.map`](#resultmap), [`Result.mapErr`](#resultmaperr), [`Result.andThen`](#resultandthen), [`Result.ok`](#resultok), [`Result.err`](#resulterr), [`Result.expect`](#resultexpect), [`Result.context`](#resultcontext)
- [Failure](#failure): [`Failure.find`](#failurefind)
- [Panic](#panic): [`Panic.resume`](#panicresume)

### Result

#### Result.isOk

```tondo
pub fn Result[T, E].isOk(self): Bool
```

Whether it holds a value.

#### Result.isErr

```tondo
pub fn Result[T, E].isErr(self): Bool
```

Whether it holds an error.

#### Result.map

```tondo
pub fn Result[T, E].map[U](self, transform: fn(T): U): U ! E
```

The function's result for the value, or the same error.

#### Result.mapErr

```tondo
pub fn Result[T, E].mapErr[F](self, transform: fn(E): F): T ! F
```

The same value, or the function's result for the error.

#### Result.andThen

```tondo
pub fn Result[T, E].andThen[U](self, next: fn(T): U ! E): U ! E
```

The result the function gives for the value, or the error.

#### Result.ok

```tondo
pub fn Result[T, E].ok(self): T?
```

The value, or `none` when it holds an error.

#### Result.err

```tondo
pub fn Result[T, E].err(self): E?
```

The error, or `none` when it holds a value.

#### Result.expect

```tondo
pub fn Result[T, E].expect(self, message: String): T
```

The value; an error panics with the message and the error.

#### Result.context

```tondo
pub fn Result[T, E].context(self, message: String): T ! Failure
```

The value, or a `Failure` that says what the program was doing, with the
error as its cause: `fs.readText(path).context("reading {path}")?`.

### Failure

```tondo
pub type Failure = {
    message: String
    // private fields
}
```

An error that says what the program was doing when it happened, with
the error that caused it (language 15.10): `reading config.yaml: no
such file`. `Failure { message: "..." }` is one with no cause.

```tondo
import std.console
import std.fs

fn load(path: String): String ! Failure {
    fs.readText(path).context("reading {path}")?
}

match load("/missing/app.toml").context("starting the server") {
    ok(text) => console.println(text)
    err(failure) => {
        console.println("{failure}")
        console.println("not found: {failure.find[fs.FsError]() == some(fs.FsError.NotFound)}")
    }
}
```

Implements `Display`.

#### Failure.find

```tondo
pub fn Failure.find[E](self): E?
```

The nearest cause of type `E` among the failure's, through the failures
that cause it: `failure.find[fs.FsError]()`.

### Panic

```tondo
pub type Panic = {
    code: String
    message: String
    data: Map[String, Int | Float | Bool | String]
    location: String
    stack: String
}
```

A panic that a `recover` stopped (15.9): its code, `P0001` or the
program's own, its message, its data, where it happened and its stack,
as its report shows them.

```tondo
import std.console

/// One job of many: a bug in it ends the job, not the worker.
fn run(job: Int): Int {
    let sizes = [10, 20]
    sizes[job]
}

for job in 0..3 {
    match recover { run(job) } {
        ok(size) => console.println("job {job}: {size}")
        err(crash) => console.println("job {job} failed with {crash.code}: {crash.message}")
    }
}
```

Implements `Display`.

#### Panic.resume

```tondo
pub fn Panic.resume(self): Never
```

Goes on with the same panic, with its code, location and stack, to the
next `recover` or the end of the program (15.9).
