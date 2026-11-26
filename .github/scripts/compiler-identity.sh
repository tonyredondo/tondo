#!/bin/sh
# `tondo compiler <revision>` gives each revision its own tondo (decision
# 267): the last revision before HEAD whose compiler's sources differ is
# built first in a fresh cache, then HEAD, then that revision again, in the
# same target after a newer one. The second build must be the first, byte
# for byte, and not HEAD's. One cache keeps every path the same, which a
# release build of tondo, reproducible, needs to give the same bytes.
#
#   .github/scripts/compiler-identity.sh <tondo>
set -eu
tondo=$1
head=$(git rev-parse HEAD)
older=$(git log -1 --format=%H HEAD~1 -- crates vendor Cargo.toml Cargo.lock)
[ -n "$older" ] || { echo "no revision before HEAD with other compiler sources: deepen the checkout"; exit 1; }
XDG_CACHE_HOME=$(mktemp -d)
export XDG_CACHE_HOME
built() { sha256sum "$("$tondo" compiler "$1" | tail -1)" | cut -d' ' -f1; }
clean=$(built "$older")
rm -r "$XDG_CACHE_HOME/tondo/compiler/$older"
newer=$(built "$head")
after=$(built "$older")
echo "$older first: $clean; HEAD $head: $newer; $older after HEAD: $after"
[ "$after" != "$newer" ] || { echo "the tondo of $older built after HEAD is HEAD's"; exit 1; }
[ "$after" = "$clean" ] || { echo "the tondo of $older built after HEAD is not its build in a fresh target"; exit 1; }
