# Projects

A single file runs as it is: `tondo run tool.to`. A project is a directory:

```text
app/
  tondo.toml          [package] name = "app"
  src/main.to         module app: fn main()
  src/models/user.to  module app.models (every file of a directory is one module)
  src/models/role.to  module app.models
  tests/api.to        an integration test root
```

```text
// src/models/user.to
pub type User = { name: String }

pub fn greet(user: User): String {
    "hello {user.name}"
}

// src/main.to
import std.console
import app.models                        // its names as models.User; or
                                         // `import app.models as m`, as m.User

fn main() {
    console.println(models.greet(models.User { name: "Ada" }))
}
```

- A module is a directory; `pub` makes a declaration visible to other
  modules. Imports name the package first: `app.models`.
- `import std.x` brings a standard module; there is nothing to install.
- Modules cannot import each other in a cycle.

## tondo.toml

```text
[package]
name = "app"

[dependencies]
units = { path = "libs/units" }
http = { git = "https://github.com/acme/http", tag = "v1.2.0" }
zlib = { url = "https://example.com/zlib-1.0.tar.gz" }

[native]
libraries = ["sqlite3"]               # C libraries for `extern fn`
```

A dependency is imported by its alias: `import units`, `import units.si`.
`tondo.lock` pins every download; `tondo update` refreshes it. `tondo api
--list` shows every module of the project, its dependencies and the
standard library.

## Editors

`tondo lsp` is a language server: diagnostics as you type, hover, go to
definition, references, rename, completion, semantic highlighting, symbols
and formatting. Point any editor's
LSP client at it for `*.to` files; in Helix, `languages.toml`:

```text
[[language]]
name = "tondo"
scope = "source.tondo"
file-types = ["to"]
roots = ["tondo.toml"]
language-servers = ["tondo"]

[language-server.tondo]
command = "tondo"
args = ["lsp"]
```

In Neovim: `vim.lsp.config('tondo', { cmd = { 'tondo', 'lsp' }, filetypes = { 'tondo' }, root_markers = { 'tondo.toml' } })`
and `vim.lsp.enable('tondo')`, with `vim.filetype.add({ extension = { to = 'tondo' } })`.
