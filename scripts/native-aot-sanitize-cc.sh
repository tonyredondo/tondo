#!/usr/bin/env bash
set -euo pipefail

# GCC's ASan global metadata records the C source path and does not honor the
# usual debug-prefix maps.  The native runner compares two fresh products, so
# feed both builds through one stable source name while preserving contents.
stable_source="/tmp/tondo-native-aot-sanitized.c"
args=()
for arg in "$@"; do
    if [[ "$arg" = /*.c && -f "$arg" ]]; then
        /bin/cp -- "$arg" "$stable_source"
        args+=("$stable_source")
    else
        args+=("$arg")
    fi
done

exec /usr/bin/cc \
    -fsanitize=address,undefined \
    -fno-sanitize=integer-divide-by-zero \
    -fno-omit-frame-pointer \
    -Wl,--build-id=none \
    "${args[@]}"
