#!/bin/sh
# What is published runs on the oldest systems it says it runs on, and so does
# what it compiles: floor.sh <arch> <directory of the tarballs>.
set -e
arch=$1
dir=$(realpath "$2")
printf 'import std.console\n\nfn main() {\n    console.println("hello")\n}\n' > "$dir/hello.to"
tar -xzf "$dir/tondo-$arch-linux-gnu.tar.gz" -C "$dir"
tar -xzf "$dir/tondo-$arch-linux-musl.tar.gz" -C "$dir"
check() {
  echo "== $2 on $1"
  docker run --rm -e HOME=/tmp -v "$dir:/built" -w /tmp "$1" sh -c "
    set -e
    tondo=/built/tondo-$arch-linux-$2/bin/tondo
    \$tondo version
    test \"\$(\$tondo run /built/hello.to)\" = hello
    \$tondo build /built/hello.to --output /tmp/hello
    test \"\$(/tmp/hello)\" = hello"
}
for image in ubuntu:22.04 debian:12 rockylinux:9; do
  check "$image" gnu
done
check rockylinux:8 musl
