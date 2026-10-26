# Install tondo

`tondo` is Tondo's whole toolchain in one executable: the compiler, the
language server, the debugger, the test runner and the tools for agents.
From a clone of [Tondo's repository](https://github.com/tonyredondo/tondo):

```sh
sh dist/install.sh
```

It builds `tondo`, or downloads it, and runs `tondo setup`, which checks
it and installs this extension in the editors it finds. The extension
finds `tondo` on `PATH`, in `~/.cargo/bin` or in `~/.local/bin`; the
setting `tondo.path` names another.

The status bar shows the version of the `tondo` found, and its menu.
