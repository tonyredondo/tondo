# The Tondo language

The whole language, a chapter at a time. Each chapter explains one part of
it with complete programs, and what they print is what they print: the
tests of the repository compile every example here and run those that show
their output.

New to Tondo? [A tour of the language](../tour.md) is the same ground in
ten minutes. Looking something up? [A quick reference](20-reference.md) has
it on a few pages.

## The basics

| Chapter | What it covers |
| --- | --- |
| 1. [Programs, scripts and the shape of a file](01-programs.md) | `main`, how a program ends, scripts, lines and statements, comments, names, keywords |
| 2. [Values, bindings and types](02-values.md) | `let` and `var`, constants, the scalar types, literals, inference, conversions, aliases and newtypes |
| 3. [Functions and closures](03-functions.md) | arguments by position, name and default, methods, constructors, closures and what they capture, functions as values |
| 4. [Control flow and operators](04-control-flow.md) | `if`, the forms of `for`, `break` and `return`, `defer`, assignment, the operators and their precedence |

## Data

| Chapter | What it covers |
| --- | --- |
| 5. [Records, tuples and newtypes](05-records.md) | fields and defaults, `with`, private fields, generic records, annotations on fields |
| 6. [Enums, unions and match](06-enums.md) | variants with data, exhaustive `match`, every kind of pattern, recursive types, `A \| B` |
| 7. [Errors](07-errors.md) | `T?`, `T ! E`, `?`, `fail`, unions of errors, `try`, `Failure`, panics, `assert`, `recover` |
| 8. [Collections](08-collections.md) | `Array`, `Map`, `Set`, `Deque`, slices, arithmetic on arrays, ranges, iterators |
| 9. [Text](09-text.md) | strings, interpolation and formats, characters, bytes, Unicode, text for people |
| 10. [Numbers](10-numbers.md) | the integers, overflow, wrapping and checked forms, floats, `Decimal`, `BigInt`, arithmetic for your types |

## Abstraction

| Chapter | What it covers |
| --- | --- |
| 11. [Generics and traits](11-generics.md) | type parameters, traits and `impl`, bounds, the prelude's traits, `impl` results, many types in one collection |
| 12. [Ownership: values, loans and identity](12-ownership.md) | what a copy costs, `ref`, `mut` and `var`, values that move, `Ref` and `WeakRef`, how memory is managed |

## Concurrency

| Chapter | What it covers |
| --- | --- |
| 13. [Tasks](13-tasks.md) | `spawn`, `Join`, `Group`, cancelling, channels, `select`, `std.sync`, pools and actors |
| 14. [Contexts and deadlines](14-contexts.md) | `context`, `with`, what a program shares, `within` |

## Programs

| Chapter | What it covers |
| --- | --- |
| 15. [Modules, packages and projects](15-modules.md) | modules and imports, `pub`, `internal`, `tondo.toml`, dependencies, files for one system or architecture, embedded files |
| 16. [Testing](16-testing.md) | `test`, `suite`, `assert`, snapshots, virtual time, tests of tasks, benchmarks, fuzzing, models |
| 17. [Metaprogramming](17-meta.md) | what the compiler writes, annotations, reflection, `derive`, generators |
| 18. [Unsafe and C](18-unsafe.md) | `unsafe`, `extern fn`, C's types, pointers, callbacks, bindings, a library for C |

## The whole

| Chapter | What it covers |
| --- | --- |
| 19. [The design, and what is not there](19-design.md) | the principles, where the ideas come from, every deliberate absence and what to use instead |
| 20. [A quick reference](20-reference.md) | every form on a few pages, and a table for who comes from another language |

## Beside this manual

- [The standard library](../std/README.md): a page for each module, with
  every declaration.
- [The toolchain](../toolchain.md): what `tondo` does besides compiling.
- [Tondo next to other languages](../comparison.md).
- `tondo guide <topic>` in a terminal: the same ground, shorter, written for
  people and for models.
- [The specifications](../../spec), which define the language and settle
  any doubt this manual leaves.
