# std.plugin

typed plugins. A trait with `@plugin.Contract` before it is a contract, and its module gets a type, `NamePlugin`, that loads an implementation of it while the program runs: from a shared library, `NamePlugin.load(path)`, or from a program of its own, `NamePlugin.start(command)`; and `serveName(value)`, with which a program serves one. What crosses is MessagePack, and the contract is checked when the plugin loads.

```tondo
import std.console
import std.plugin

/// What a plugin offers here.
@plugin.Contract
pub trait Greeter {
    fn greet(self, name: String): String ! plugin.Error
}

type Polite = {}

impl Greeter for Polite {
    fn greet(self, name: String): String ! plugin.Error {
        "good morning, {name}"
    }
}

fn main() {
    let polite = Polite {}
    match polite.greet("Ada") {
        ok(text) => console.println(text)
        err(error) => console.println("{error}")
    }
}
```

<!-- From here on, docs/std/generate.to writes this page from `tondo api`: edit above this line. -->

## Reference

`tondo api std.plugin` shows the same in a terminal.

- [Functions](#functions): [`described`](#described), [`contractText`](#contracttext), [`decoded`](#decoded), [`nothing`](#nothing), [`Returned`](#returned), [`Failed`](#failed), [`Broken`](#broken), [`serve`](#serve), [`answered`](#answered), [`broken`](#broken-1), [`argument`](#argument), [`offer`](#offer), [`offered`](#offered), [`release`](#release)
- [Contract](#contract)
- [Export](#export)
- [Error](#error)
- [Description](#description)
- [Link](#link): [`Link.load`](#linkload), [`Link.start`](#linkstart), [`Link.call`](#linkcall)

### Functions

#### described

```tondo
pub fn described(name: String, text: String, methods: Array[String]): Description
```

The description of a contract, its fingerprint the SHA-256 of `text`,
the contract written out in full, which the checker gives
`contractText` (14.48).

#### contractText

```tondo
pub fn contractText[P](): String
```

The contract of `NamePlugin`, the trait `Name`, written out in full: the
checker puts the text in place of the call.

#### decoded

```tondo
pub fn decoded[T: serialization.Decode](bytes: Array[Byte]): T ! Error
```

The value in `bytes`, or why it is not a `T`.

#### nothing

```tondo
pub fn nothing(): Array[Byte]
```

The bytes of a method's answer that gives nothing: MessagePack's nil.

#### Returned

```tondo
pub const Returned: Int = 0
```

The status of an answer that holds the method's result (14.48).

#### Failed

```tondo
pub const Failed: Int = 1
```

The status of an answer that holds the method's own error.

#### Broken

```tondo
pub const Broken: Int = 2
```

The status of an answer that holds a `plugin.Error`.

#### serve

```tondo
pub fn serve(contract: Description, answer: fn(Int, Array[Byte]): (Int, Array[Byte])): !Error
```

Serves a contract over this program's standard input and output: its
contract first, then an answer for each request, each request in a task
of its own, until the other end closes. For the declarations a contract
makes: a program calls `serveName`.

#### answered

```tondo
pub fn answered(
    answer: fn(Int, Array[Byte]): (Int, Array[Byte]),
    method: Int,
    given: Array[Byte],
): (Int, Array[Byte])
```

The answer of a method, with a panic of it as `Panicked`.

#### broken

```tondo
pub fn broken(error: Error): (Int, Array[Byte])
```

The bytes of a failure with `error`, for a method that cannot read its
arguments.

#### argument

```tondo
pub fn argument[T: serialization.Decode](given: Array[Byte], index: Int): T ! Error
```

Argument number `index` of `arguments`, MessagePack of an array of them.

#### offer

```tondo
pub unsafe fn offer(
    contract: Description,
    answer: fn(Int, Array[Byte]): (Int, Array[Byte]),
    bytes: Pointer[Pointer[Byte]],
    length: Pointer[ffi.CSize],
)
```

A library's `tondo_plugin_contract` (14.48): writes the contract's bytes
where `bytes` and `length` point, which `release` frees, and keeps
`answer`, the first time, for the calls.

#### offered

```tondo
pub unsafe fn offered(
    method: UInt32,
    input: Pointer[Byte],
    inputLength: ffi.CSize,
    output: Pointer[Pointer[Byte]],
    length: Pointer[ffi.CSize],
): Int32
```

A library's `tondo_plugin_call` (14.48): the answer of method `method`
to the bytes at `input`, written where `output` and `length` point; its
status is what the call gives.

#### release

```tondo
pub unsafe fn release(bytes: Pointer[Byte], length: ffi.CSize)
```

A library's `tondo_plugin_free` (14.48): frees what `offer` or `offered`
wrote.

### Contract

```tondo
pub annotation Contract = {}
```

Marks a trait as a contract (14.48): each method takes `self`, has no
type parameters, and gives `T ! Error` or `T ! (E | Error)`, of types
that are written and read.

```tondo
import std.plugin

@plugin.Contract
pub trait Counter {
    fn count(self, text: String): Int ! plugin.Error
}
```

### Export

```tondo
pub annotation Export = {}
```

Marks the function of a library that gives the implementation of a
contract it offers: no parameters, and `impl Name` of a type that is
`Send` and `Share`.

```tondo
import std.plugin

@plugin.Contract
pub trait Counter {
    fn count(self, text: String): Int ! plugin.Error
}

type Words = {}

impl Counter for Words {
    fn count(self, text: String): Int ! plugin.Error {
        text.split(" ").length()
    }
}

@plugin.Export
fn counter(): impl Counter {
    Words {}
}
```

### Error

```tondo
pub enum Error {
    /// The library or the program could not be loaded or started, with why.
    NotLoaded(String)
    /// The plugin was built against another contract, with what differs.
    Contract(String)
    /// The plugin panicked in a call, with the panic's message.
    Panicked(String)
    /// What crossed could not be read, with why.
    Format(String)
    /// The plugin is gone: unloaded, or its program ended.
    Closed
}
```

What can go wrong with a plugin, beside a method's own errors.

```tondo
import std.console
import std.plugin

console.println("{plugin.Error.NotLoaded("no such file")}")
```

Implements `Display`, `reflect.Reflect`, `serialization.Encode`, `serialization.Decode`.

### Description

```tondo
pub type Description = {
    name: String
    fingerprint: String
    methods: Array[String]
}
```

What a contract is, as both ends compare it: its module and name, the
fingerprint of its methods' types (14.48), and its methods' names in
order. For the declarations a contract makes.

```tondo
import std.console
import std.plugin

@plugin.Contract
pub trait Counter {
    fn count(self, text: String): Int ! plugin.Error
}

console.println("{CounterPlugin.contract().methods}")
```

### Link

```tondo
pub type Link = {
    // private fields
}
```

The other end of a contract's plugin. For the declarations a contract
makes: a program calls the methods of `NamePlugin`, which holds one.

```tondo
import std.console
import std.plugin

@plugin.Contract
pub trait Counter {
    fn count(self, text: String): Int ! plugin.Error
}

match CounterPlugin.load("plugins/missing.so") {
    ok(_) => console.println("loaded")
    err(error) => console.println("{error}")
}
```

#### Link.load

```tondo
pub fn Link.load(path: String, contract: Description): Link ! Error
```

The library at `path`, which must offer `contract`.

#### Link.start

```tondo
pub fn Link.start(command: Command, contract: Description): Link ! Error
```

Starts `command`, which must serve `contract` over its standard input
and output.

#### Link.call

```tondo
pub fn Link.call(self, method: Int, input: Array[Byte]): (Int, Array[Byte]) ! Error
```

Calls method number `method` with its arguments' bytes: the status and
the bytes of the answer, or the `plugin.Error` the other end gave.
