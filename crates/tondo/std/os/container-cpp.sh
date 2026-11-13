#!/bin/sh
# The C preprocessor of another architecture's Debian, in a container, for
# generate.to: `TONDO_CC=crates/tondo/std/os/container-cpp.sh` with
# `TONDO_CONTAINER=<image>` and `TONDO_PLATFORM=linux/arm64`. The headers
# come in as the paths of this machine and go out as the same paths, so
# that `tondo bind` knows them, where Debian keeps an architecture's headers
# in a directory of its own.
set -e
sed -E 's|#include "/usr/include/([^"]*)"|#include <\1>|' |
    docker run --rm -i --platform "${TONDO_PLATFORM:-linux/arm64}" "$TONDO_CONTAINER" gcc "$@" |
    sed -E 's|/usr/include/[a-z0-9_]+-linux-gnu/|/usr/include/|g'
