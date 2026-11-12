# The design, and what is not there

A language is as much what it leaves out as what it has. This chapter says
what Tondo is trying to be, what it took from the languages before it, and
what it deliberately does not have, with what to use instead.

- [What Tondo is for](#what-tondo-is-for)
- [The principles](#the-principles)
- [A program as closed contracts](#a-program-as-closed-contracts)
- [Where the ideas come from](#where-the-ideas-come-from)
- [What is not there](#what-is-not-there)
- [What Tondo does not try to be](#what-tondo-does-not-try-to-be)

## What Tondo is for

Tondo is a general language for programs that run on a server or at a
command line: services, tools, pipelines, scripts that grew. It is meant to
be small enough to hold in one's head, safe by default, and fast enough that
nothing has to be rewritten in another language later.

It is also written for a time when much code is written, read and changed
by models as well as by people. That shows in choices that have nothing to
do with any one model: few ways to say a thing, signatures that carry the
whole contract, one canonical layout, and errors that say what to write.
What is easy for a model to get right is easy for a person too.

Its motto is the one of its specification: **small by design, complete in
practice.**

## The principles

1. **One way to say each thing.** One loop, one conditional, one way to
   tell cases apart, one way to export a name, one way to fail. Where two
   forms would do the same job, there is one.
2. **A signature is the whole contract.** What a function takes, what it
   gives, what it fails with, and what it may change in its caller's values
   are all in its first line. Nothing is thrown past it and nothing is
   inferred across functions.
3. **Inference is local.** Types are worked out inside a function and
   written between functions. Understanding a function takes its body, its
   signature and what it imports.
4. **Errors are values.** What can go wrong is in the type, and the caller
   has to deal with it. A bug is something else, and stops the program.
5. **Values by default, and immutable by default.** Assigning copies, `let`
   does not change, and nothing is shared that was not asked for.
6. **Mutation is visible and temporary.** `var` at the binding, `mut` and
   `var` at the call. A function that can change its argument says so, and
   so does whoever calls it.
7. **Order is defined, and tasks are structured.** Evaluation goes left to
   right; a task belongs to the block that started it.
8. **A grammar that is easy to parse and to format.** No semicolons to
   forget, no ambiguity to resolve, one layout that a tool writes.
9. **Diagnostics a tool can use.** Every error has a stable code, an exact
   place, and where the compiler knows it, the edit that fixes it.
10. **No behaviour hidden in the environment.** The same code means the same
    in a debug build and a release one, on every machine. Importing a
    module runs nothing.

## A program as closed contracts

The specification states what a faithful Tondo program looks like. Each
thing a reader might need to know is said by one construct, and can be found
by looking for it:

| What | Is seen as |
| --- | --- |
| The data that can exist | types: records and enums |
| A value that may be missing | `?` after a type |
| A function that may fail | `!` in its signature |
| A value that changes | `var`, or `mut` |
| Something shared with an identity | `Ref` |
| A value lent for a call | `ref`, `mut`, `var` at the call |
| Raw access to memory | `Pointer`, inside `unsafe` |
| Work done at the same time | `spawn`, and `wait()` where its result is taken |
| What a module offers | `pub` |
| Telling cases apart | `match` |
| Cleanup | `defer` |
| Repetition | `for` |
| Where effects begin | `main`, or the top of a script |

The language does not get short by hiding information. It gets short by
removing ceremony that a single rule can supply.

## Where the ideas come from

Tondo did not start from a blank page. Its specification names what it took
from each language, and what it left:

| From | It took | And left |
| --- | --- | --- |
| **Go** | a small core, one loop, explicit imports, an official formatter, composition, typed channels and `select`, cheap tasks | `nil`, errors checked by hand, visibility by capitals, implicit interfaces, tasks nobody owns |
| **Rust** | `Option` and `Result`, enums with data, exhaustive `match`, no null, explicit mutability, traits with explicit `impl`s, panics apart from errors | lifetimes in signatures, a general borrow checker, macros in daily use, a trait for each operator |
| **Kotlin** | `if` and `match` as expressions, interpolated strings, `T?`, named arguments, structured concurrency | classes and inheritance, nullable references, overloading, exceptions |
| **Swift** and **Zig** | a task that belongs to the block that owns it, reference counting with copy on write | coloured functions, allocators in everyday signatures, cycles broken by hand |
| **C#** | lending a value for a call, to read or to change; views over contiguous memory | the class hierarchy, `null`, `Task<T>` in every signature |
| **Python** and **NumPy** | clear collection literals, negative indexes, slices, maps in insertion order, arithmetic on arrays | dynamic typing, truthiness, `+` for joining lists |
| **Odin** | plain data and procedures, `defer`, a small unsafe boundary, a context that goes with the work | manual memory in ordinary code |
| **Lua** | reading that is direct, and little ceremony for a small program | dynamic typing, one table for everything |
| **Bash** and **Nushell** | small programs run as scripts, pipelines of processes with `\|` | commands as strings, hidden expansion and quoting |

[Tondo next to other languages](../comparison.md) goes through eight of
them in detail, with the same programs in both.

## What is not there

Each absence removes a second way to do something, or a way for something
to happen without being written.

### Values and types

| Not in Tondo | Why | Instead |
| --- | --- | --- |
| `null` | any value could be missing without saying so | `T?`, which must be opened |
| A universal type (`Any`, `Object`) | it defers every check to run time | enums, unions, generics; `json.Value` for data with no schema |
| Classes and inheritance | behaviour and data tangled across a hierarchy | records, enums, traits, composition |
| Implicit conversions | a value changes type where nobody wrote it | `Float(n)`, `Int(x)?`, `"{n}"` |
| Truthiness | `if x` means a different thing for each type | a `Bool`: `x != 0`, `not items.isEmpty()` |
| Uninitialized variables | a value that is not one yet | a binding always starts with a value |
| Shadowing | one name, two meanings, a few lines apart | another name |
| Array lengths in types | generics over values, and their rules | `Array[T]`, whose length is checked when used |
| Several kinds of the same collection | `List`, `Vector`, `Slice`, `Dictionary` | `Array`, `Map`, `Set`, `Deque` |

### Functions and errors

| Not in Tondo | Why | Instead |
| --- | --- | --- |
| Exceptions | a function fails in ways its signature does not show | `T ! E`, `?`, `match` |
| Overloaded functions | which one is called depends on conversions | one name, one function; generics, defaults, another name |
| Overloaded operators | `a + b` could run anything | `Additive` and `Number` for arithmetic; named methods |
| A ternary operator, an Elvis operator | a second conditional | `if` as an expression, `unwrapOr` |
| Safe navigation (`?.`) | absence decided inside a chain | `?` in a `try` block, `map`, `andThen` |
| `+` for joining strings and lists | one operator, three meanings | interpolation, `concat`, `join` |

### Programs

| Not in Tondo | Why | Instead |
| --- | --- | --- |
| Mutable globals | state that any code can change at any time | contexts, and `std.sync` passed where it is used |
| Initializers that run on import | an order of start-up nobody wrote | `main`, and context fields computed when first read |
| Destructors and finalizers | code that runs at a moment nobody chose | `defer`, and resources that close when their owner ends |
| Macros and a preprocessor | code that is not what is written | `derive` and generators, whose output can be printed |
| Attributes that change compilation, `#if` | a declaration that means something else under a flag | files for a system; annotations, which are only data |
| A build that behaves differently when optimized | a bug that exists only in production | the same checks and results in every build |
| Visibility by capitals | a rename that changes who sees a name | `pub` |
| Semicolons, and indentation that means something | noise, or structure that cannot be seen | new lines and braces |

### Concurrency and memory

| Not in Tondo | Why | Instead |
| --- | --- | --- |
| `async` and `await` | two kinds of function, and libraries in two versions | any function waits |
| Futures and promises | a result that may never be asked for | a `Join`, which its block waits for |
| Tasks nobody owns | work that outlives whoever started it | structured tasks; `spawn detached` when that is meant |
| Generators with `yield` | a second kind of function | `Iterator[T]` |
| Trait objects | calls resolved as the program runs, and how long what they borrow lives | enums, unions, functions as values |
| Lifetimes | annotations for how long a reference is good | loans that last one call; values |
| Pointers in ordinary code | memory read wrongly | values, `Ref[T]`; `Pointer[T]` inside `unsafe` |
| Several loops | `while`, `do`, `loop`, C's `for` | `for` |

## What Tondo does not try to be

- **Compatible with another language's syntax.** It reads like its
  relatives, and is none of them. `tondo guide pitfalls` lists what each
  habit becomes.
- **A systems language for code without a runtime.** A Tondo program has a
  scheduler, automatic memory and a standard library in it. For a kernel or
  a microcontroller, a language where each allocation is placed by hand is
  the right tool.
- **A language with every feature.** A feature that would add a second way
  to do something is weighed against the first way, and usually loses.
- **Finished.** It is in early development, and what is here can still
  change. Changes to the language go through its specification first, which
  is why the manual and the compiler agree.

Next: [A quick reference](20-reference.md).
