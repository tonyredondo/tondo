#!/bin/sh
# Builds the release `tondo` for this machine, on macOS signs and notarizes
# it when the environment gives the certificate (dist/sign-macos.sh),
# checks that it compiles and runs a program, and packs it as
# dist/tondo-<target>.tar.gz.
set -eu
target="$1"
name="tondo-$target"
cargo build --release --locked -p tondo
rm -rf "dist/$name" "dist/$name.tar.gz"
mkdir -p "dist/$name/bin"
cp target/release/tondo "dist/$name/bin/"
if [ "$(uname -s)" = Darwin ]; then
    sh dist/sign-macos.sh "dist/$name/bin/tondo"
fi
cp dist/README.md "dist/$name/"
printf 'import std.console\n\nconsole.println("hello from tondo")\n' > "dist/$name/hello.to"
test "$("dist/$name/bin/tondo" run "dist/$name/hello.to")" = "hello from tondo"
rm "dist/$name/hello.to"
tar -C dist -czf "dist/$name.tar.gz" "$name"
