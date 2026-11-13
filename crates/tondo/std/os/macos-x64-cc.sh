#!/bin/sh
# The C preprocessor of macOS for x86-64, on a Mac of either architecture,
# for generate.to: `TONDO_CC=crates/tondo/std/os/macos-x64-cc.sh`. Both
# architectures are generated from one SDK, so that they differ by the
# architecture alone.
exec cc -arch x86_64 "$@"
