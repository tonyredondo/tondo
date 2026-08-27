#!/usr/bin/env bash
set -euo pipefail

exec /usr/bin/cc \
    -fsanitize=address,undefined \
    -fno-omit-frame-pointer \
    "$@"
