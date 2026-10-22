#!/bin/sh
# Builds the release `tondo` for this machine, checks that it compiles and
# runs a program, and packs it as dist/tondo-<target>.tar.gz.
set -eu
target="$1"
name="tondo-$target"
cargo build --release --locked -p tondo
rm -rf "dist/$name" "dist/$name.tar.gz"
mkdir -p "dist/$name/bin"
cp target/release/tondo "dist/$name/bin/"
cp dist/README.md "dist/$name/"
printf 'import std.console\n\nconsole.println("hello from tondo")\n' > "dist/$name/hello.to"
test "$("dist/$name/bin/tondo" run "dist/$name/hello.to")" = "hello from tondo"
rm "dist/$name/hello.to"
tar -C dist -czf "dist/$name.tar.gz" "$name"
