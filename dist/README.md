# Tondo

`bin/tondo` is the whole toolchain: the compiler, its runtime, the standard
library and the test runner. Put `bin` on your `PATH`:

```sh
export PATH="$PWD/bin:$PATH"
tondo version
```

It needs a C compiler to link executables: `cc` by default, or the one
`TONDO_CC` names (Xcode's command line tools on macOS; `gcc` or `clang` with
the C library's headers on Linux, `musl-dev` on Alpine). Dependencies from git
or archives use `git`, `curl`, `tar` and `unzip`; `tondo bind` uses
`pkg-config` when it is given a package.

```sh
tondo run hello.to          # compile and run a file
tondo build app --output app # a native executable from a project
tondo test app              # run a project's tests
tondo help                  # every command
```
