# Make a project

*Tondo: New Project* asks for a name and a folder, and makes there:

```
tondo.toml          the project's name and dependencies
src/main.to         the program
src/main_test.to    a test of it
.gitignore
```

`tondo new <dir>` does the same from a terminal, and `tondo init` in a
folder that has files already. `tondo add <name> --path <dir>` adds a
dependency, or `--git <url>`.

A file alone, without a project, runs as a script.
