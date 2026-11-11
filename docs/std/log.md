# std.log

structured logging, a line per event, as text or JSON, through a logger or the one of the context.

```tondo
import std.log

log.info("server started", ["port": "8080"])
log.warn("disk almost full", ["free": "3%"])
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.log` shows the same in a terminal.

- [Functions](#functions): [`current`](#current), [`trace`](#trace), [`debug`](#debug), [`info`](#info), [`warn`](#warn), [`error`](#error)
- [Level](#level)
- [Format](#format)
- [Logger](#logger): [`Logger`](#logger-1), [`Logger.withFields`](#loggerwithfields), [`Logger.isEnabled`](#loggerisenabled), [`Logger.log`](#loggerlog), [`Logger.trace`](#loggertrace), [`Logger.debug`](#loggerdebug), [`Logger.info`](#loggerinfo), [`Logger.warn`](#loggerwarn), [`Logger.error`](#loggererror), [`Logger.flush`](#loggerflush)
- [Context](#context)

### Functions

#### current

```tondo
pub fn current(): Logger
```

The logger of `Context`, or without one a logger as `Logger()`,
which writes to standard error from `Info` on.

#### trace

```tondo
pub fn trace(message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Trace` with the logger of `Context`.

#### debug

```tondo
pub fn debug(message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Debug` with the logger of `Context`.

#### info

```tondo
pub fn info(message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Info` with the logger of `Context`.

#### warn

```tondo
pub fn warn(message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Warn` with the logger of `Context`.

#### error

```tondo
pub fn error(message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Error` with the logger of `Context`.

### Level

```tondo
pub enum Level {
    Trace
    Debug
    Info
    Warn
    Error
}
```

How much an event matters, from least to most.

```tondo
import std.log

let logger = log.Logger(level: log.Level.Debug)
if logger.isEnabled(log.Level.Debug) {
    logger.debug("details", ["step": "2"])
}
```

Implements `Display`.

### Format

```tondo
pub enum Format {
    Text
    Json
}
```

`Text` writes `time LEVEL message key=value ...`, quoting values with
spaces; `Json` writes an object per line.

```tondo
import std.log

// One JSON object a line, for a collector of logs.
let logger = log.Logger(format: log.Format.Json)
logger.info("started", ["port": "8080"])
```

### Logger

```tondo
pub type Logger = {
    // private fields
}
```

Writes events of its level or above, with its fields: a program makes
one and passes it on, or puts it in `Context`.

```tondo
import std.log

let logger = log.Logger().withFields(["service": "billing"])
logger.info("invoice sent", ["id": "42"])
logger.flush()?
```

#### Logger

```tondo
pub fn Logger(
    output: io.Writer? = none,
    level: Level = Level.Info,
    format: Format = Format.Text,
    timestamps: Bool = true,
): Logger
```

A logger to `output`, or to standard error without one.
`timestamps: false` leaves out the time, for output that tests compare.

#### Logger.withFields

```tondo
pub fn Logger.withFields(self, fields: Map[String, String]): Logger
```

The same logger with more fields in every event it writes.

#### Logger.isEnabled

```tondo
pub fn Logger.isEnabled(self, level: Level): Bool
```

Whether events of this level are written: to skip building costly
fields.

#### Logger.log

```tondo
pub fn Logger.log(self, level: Level, message: String, fields: Map[String, String] = [:])
```

Writes an event, if its level is enabled. A stream that fails loses the
event; `flush` reports it.

#### Logger.trace

```tondo
pub fn Logger.trace(self, message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Trace`, with its fields, if the logger takes
that level.

#### Logger.debug

```tondo
pub fn Logger.debug(self, message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Debug`, with its fields, if the logger takes
that level.

#### Logger.info

```tondo
pub fn Logger.info(self, message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Info`, with its fields, if the logger takes
that level.

#### Logger.warn

```tondo
pub fn Logger.warn(self, message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Warn`, with its fields, if the logger takes
that level.

#### Logger.error

```tondo
pub fn Logger.error(self, message: String, fields: Map[String, String] = [:])
```

Writes the event at level `Error`, with its fields.

#### Logger.flush

```tondo
pub fn Logger.flush(self): !io.IoError
```

Hands what was written to the stream's host.

### Context

```tondo
pub context Context = {
    logger: Logger? = none
}
```

The logger of the work that runs, which the functions of the module
write with: `main` puts one there, and code below adds its fields.

```tondo
import std.log

fn work() {
    // The logger of the context, with the request's fields.
    log.info("working")
}

with log.Context { logger: some(log.Logger().withFields(["request": "r1"])) }
work()
```
