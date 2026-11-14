# tabulate: a program and its plugins

`tabulate` shows a CSV file as a table, and what draws the table is a
renderer. One is part of the program. The others are plugins it finds in a
directory when it starts: a shared library and a program, each a package of
its own that knows nothing of `tabulate` but the contract they share.

```text
contract/    renderer.api: the table, and the trait a renderer implements
host/        tabulate: reads the CSV, loads the plugins, asks the one named
markdown/    a renderer built as a library
json/        a renderer built as a program
```

## Run it

```text
$ sh build.sh
--- plain
sales.csv
region  product  amount
north   tea      120.50
south   tea      80.00
north   cups     45.25
--- markdown
## sales.csv

| region | product | amount |
| --- | --- | --- |
| north | tea | 120.50 |
| south | tea | 80.00 |
| north | cups | 45.25 |
--- json
[
  {
    "region": "north",
    "product": "tea",
    "amount": "120.50"
  },
  ...
]
```

`build.sh` builds the program, the library and the plugin program into
`out/`, and runs `out/tabulate sales.csv <renderer> out/plugins` for each
renderer. `tondo test` in each directory runs that package's tests.

## What to look at

**The contract is a trait.** [`api.to`](contract/src/api/api.to) declares
`Renderer` with `@plugin.Contract` before it, and the type its methods
take. That is all a plugin and a program share. Each method takes `self`
and gives its result or a `plugin.Error`, since a plugin can be gone when
it is called.

**A plugin implements the trait, and says how it is offered.**
[`markdown.to`](markdown/src/markdown.to) is built with `tondo build
--library`, and its function marked `@plugin.Export` gives the value the
library offers. [`json`](json/src/main.to) is a program: its `main` serves
the contract with `api.serveRenderer` until the other side closes. The
code that renders is the same kind of code in both.

**The program loads either with one line.** The contract's module gets a
type the compiler writes, `api.RendererPlugin`, which implements `Renderer`
by calling the plugin. In [`main.to`](host/src/main.to),
`api.RendererPlugin.load(file)` loads a library and
`api.RendererPlugin.start(process.command(file))` starts a program. What
crosses between the two is MessagePack, written and read by the compiler's
own code for the types of the contract: there is no schema to keep and
nothing to generate.

**A plugin is a `Renderer` like any other.** `shown` takes any
`api.Renderer`, so the program's own `Plain` and a loaded plugin go through
the same function, and the tests of `tabulate` use `Plain` with no plugin
built.

**A plugin of another contract is refused.** Loading compares a
fingerprint of the contract, its methods and every type they reach. A
plugin built before a field was added to `Table` gives
`plugin.Error.Contract` when it is loaded, in words that name both sides,
and never a table read wrong.

**Each side is tested alone.** The renderers are tested by calling them as
the trait, and [`api_test.to`](contract/src/api/api_test.to) checks that a
table arrives as it was sent.

## Going further

- A library plugin runs in the program's process: a call writes its values,
  calls a function and reads the answer. A program plugin is another
  process, slower to call, and a crash or a leak in it stays there.
  `plugin.Error` says what happened: `Panicked`, `Closed`.
- A `RendererPlugin` is shared by tasks. When its last copy goes, the
  program it started ends, and the library it loaded is ended and unloaded.
