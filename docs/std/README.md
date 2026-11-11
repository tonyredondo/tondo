# The standard library

Everything here comes with `tondo`: there is nothing to install and no
version to choose. A module is brought with `import std.name`, and the
prelude, the types and functions every program starts with, needs no
import.

Each page below has every public declaration of its module, with its
documentation and examples. The pages are written from the library's own
sources by [`generate.to`](generate.to), and a test holds them to what
`tondo api` gives, so they cannot fall behind. In a terminal,
`tondo api std.http` shows the same, `tondo api std.http.Client` one type,
and `tondo api --search <words>` finds a declaration by what it does.

- [The prelude](#the-prelude)
- [Web](#web)
- [Data formats](#data-formats)
- [Databases](#databases)
- [Network and security](#network-and-security)
- [Files and the system](#files-and-the-system)
- [Text, numbers and time](#text-numbers-and-time)
- [Concurrency](#concurrency)
- [Programs and their terminal](#programs-and-their-terminal)
- [Testing and looking inside](#testing-and-looking-inside)
- [The language's own machinery](#the-languages-own-machinery)

## The prelude

What needs no import. [The language manual](../language/README.md) explains
these types; the pages list their methods.

| Page | What it has |
| --- | --- |
| [String](prelude/string.md) | text: splitting, trimming, searching, case, padding |
| [Char and Byte](prelude/char.md) | one Unicode scalar, and one byte of data |
| [Bool](prelude/bool.md) | `true` and `false` |
| [Int and the integers](prelude/int.md) | every width, with wrapping, checked and saturating arithmetic, bits and bytes |
| [Float and Float32](prelude/float.md) | rounding, roots, powers, tests for NaN and infinity |
| [Option](prelude/option.md) | `T?`: `map`, `andThen`, `unwrapOr`, `okOr` |
| [Result, Failure and Panic](prelude/result.md) | `T ! E`: `map`, `mapErr`, `context`; what a `recover` gives |
| [Array](prelude/array.md) | `map`, `filter`, `reduce`, sorting, grouping, slices |
| [Map](prelude/map.md) | keys in the order they were added |
| [Set and Deque](prelude/set.md) | unique values, and a queue with two ends |
| [Range and Iterator](prelude/range.md) | `0..10`, and the lazy adapters every iterator has |
| [Join, Group and channels](prelude/tasks.md) | what `spawn` gives, groups of tasks, `Sender` and `Receiver` |
| [Ref, WeakRef and Contexts](prelude/ref.md) | identity, and every context as a value |
| [The prelude's traits](prelude/traits.md) | `Ordered`, `Additive`, `Number`, `Integer` |

## Web

| Module | What it is for |
| --- | --- |
| [std.http](http.md) | a client and a server for HTTP/1.1 and HTTP/2 over TLS: typed handlers, middleware, files, uploads, WebSockets, Server-Sent Events, OpenAPI |
| [std.grpc](grpc.md) | gRPC servers and clients from the services of a `.proto` file, with streams, deadlines, metadata, gRPC-Web and Connect |
| [std.graphql](graphql.md) | a GraphQL server made from the program's types |
| [std.template](template.md) | text and HTML templates whose expressions are Tondo's, compiled with the program or loaded as it runs |
| [std.url](url.md) | URLs and relative references: their parts, resolving, the query of a form |
| [std.acme](acme.md) | certificates of Let's Encrypt, obtained and renewed by the server itself |
| [std.mcp](mcp.md) | a Model Context Protocol server from the methods of a value |

## Data formats

The compiler writes each type's encoder and decoder once, and every format
uses them: the same record is read from JSON and written as YAML.

| Module | What it is for |
| --- | --- |
| [std.json](json.md) | JSON to and from any type, dynamic values, streams of documents |
| [std.yaml](yaml.md) | YAML, and a document edited in place that keeps its comments |
| [std.toml](toml.md) | TOML, with its dates |
| [std.xml](xml.md) | XML to and from types, with attributes by annotation, and as a tree |
| [std.csv](csv.md) | CSV a record at a time, so a file of any size streams through |
| [std.cbor](cbor.md) | CBOR, deterministic when asked |
| [std.messagepack](messagepack.md) | MessagePack |
| [std.protobuf](protobuf.md) | Protocol Buffers, proto2 and proto3, from records whose fields carry numbers |
| [std.serialization](serialization.md) | the protocol all of them share, for a format or an encoding of your own |
| [std.validate](validate.md) | rules on a type's fields that no value read from outside breaks |
| [std.encoding](encoding.md) | Base64, hexadecimal, URL text, HTML escaping |

## Databases

| Module | What it is for |
| --- | --- |
| [std.sql](sql.md) | SQLite, PostgreSQL and MySQL through one API: queries whose values are parameters, rows read into types, transactions, migrations, a pool of connections |

## Network and security

| Module | What it is for |
| --- | --- |
| [std.net](net.md) | TCP, UDP and Unix sockets, TLS, names, IP addresses and networks |
| [std.crypto](crypto.md) | passwords, keys derived from secrets, sealing with a key, signatures, PEM keys and certificates |
| [std.hash](hash.md) | SHA-2, SHA-1, MD5 and HMAC, whole or in parts; FNV, xxHash and MurmurHash3 |
| [std.uuid](uuid.md) | UUIDs, versions 4, 5 and 7 |
| [std.random](random.md) | generators, seeded or from the system's entropy |

## Files and the system

| Module | What it is for |
| --- | --- |
| [std.fs](fs.md) | reading and writing files, walking and watching directories, metadata, temporary files, trees of files of any origin |
| [std.path](path.md) | joining, splitting and cleaning paths, glob patterns |
| [std.io](io.md) | the streams the console, files, sockets and processes share: `Reader`, `Writer`, `Buffer`, pipes |
| [std.archive](archive.md) | zip, tar, tar.gz and tar.zst: read, write, extract |
| [std.gzip](gzip.md), [std.zstd](zstd.md) | compression, whole or as streams |
| [std.embed](embed.md) | files of the package kept inside the executable |
| [std.image](image.md) | PNG, JPEG, GIF and WebP: resize, crop, rotate, colors, EXIF |
| [std.process](process.md) | running programs and pipelines, their input and output, signals |
| [std.env](env.md) | arguments, environment, directories, processors, what the program was built from |

## Text, numbers and time

| Module | What it is for |
| --- | --- |
| [std.regex](regex.md) | regular expressions that run in linear time |
| [std.text](text.md) | Unicode normalization, categories and scripts, the order of each language |
| [std.format](format.md) | a text builder, formats chosen at run time, numbers and plurals as each language writes them |
| [std.math](math.md) | mathematics on floats, `BigInt` and exact `Decimal` |
| [std.time](time.md) | durations, the clock, dates, calendars, time zones, deadlines |
| [std.collections](collections.md) | the error of collection operations; the collections themselves are in the prelude |

## Concurrency

`spawn`, `Group`, channels and `select` are the language's
([Tasks](../language/13-tasks.md)). These modules add the rest.

| Module | What it is for |
| --- | --- |
| [std.sync](sync.md) | `Mutex`, `RwLock`, `Atomic`, `Semaphore`, `Once`, `Barrier`, shared collections, rate limits, retries |
| [std.executor](executor.md) | pools that bound work, and actors with a mailbox |

## Programs and their terminal

| Module | What it is for |
| --- | --- |
| [std.console](console.md) | standard output, error and input |
| [std.cli](cli.md) | a command line read into types, with its help; styles, tables, progress bars, questions |
| [std.log](log.md) | structured logs, as text or JSON |

## Testing and looking inside

| Module | What it is for |
| --- | --- |
| [std.testing](testing.md) | what a test tells the runner: snapshots, virtual time, benchmarks, fuzzing |
| [std.model](model.md) | every state of a design or of the code, and the rules it must keep |
| [std.profile](profile.md) | profiles a program makes of itself, in pprof's format |
| [std.diagnostics](diagnostics.md) | the program's own counters, and the stack of a task |
| [std.debug](debug.md) | how a type shows in the debugger |

## The language's own machinery

| Module | What it is for |
| --- | --- |
| [std.reflect](reflect.md) | values and types by name, as the program runs |
| [std.meta](meta.md) | what a `derive` provider or a generator receives and writes |
| [std.ffi](ffi.md) | C's types, memory and strings, for calling C |
