#!/bin/sh
# `tondo compiler <revision>` gives each revision its own tondo (decision
# 267): the revision before the last commit that changed what tondo is built
# from, whose tondo is therefore another, is built first in a fresh cache,
# then HEAD, then that revision again, in the same target after a newer one.
# The second build must be the first, byte for byte, and not HEAD's. One
# cache keeps every path the same, which a release build of tondo,
# reproducible, needs to give the same bytes.
#
#   .github/scripts/compiler-identity.sh <tondo>
set -eu
tondo=$1
head=$(git rev-parse HEAD)
# What a release build of tondo reads: its sources and those it embeds, its
# dependencies and its toolchain; not the tests.
last=$(git log -1 --format=%H HEAD -- crates vendor conformance Cargo.toml Cargo.lock \
    rust-toolchain.toml ':(exclude)crates/tondo/tests')
if [ -z "$last" ] || ! older=$(git rev-parse -q --verify "$last~1^{commit}"); then
    echo "no revision before the last that changed tondo's sources: deepen the checkout"
    exit 1
fi
XDG_CACHE_HOME=$(mktemp -d)
export XDG_CACHE_HOME
built() {
    "$tondo" compiler "$1" > "$XDG_CACHE_HOME/given" || exit 1
    path=$(tail -1 "$XDG_CACHE_HOME/given")
    [ -f "$path" ] || { echo "tondo compiler $1 gave no tondo: $path" >&2; exit 1; }
    sha256sum "$path" | cut -d' ' -f1
}
clean=$(built "$older")
rm -r "$XDG_CACHE_HOME/tondo/compiler/$older"
newer=$(built "$head")
after=$(built "$older")
echo "$older first: $clean; HEAD $head: $newer; $older after HEAD: $after"
[ "$clean" != "$newer" ] || { echo "HEAD's tondo is $older's: HEAD's build was stale, or the two build the same and this cannot tell them apart"; exit 1; }
[ "$after" != "$newer" ] || { echo "the tondo of $older built after HEAD is HEAD's"; exit 1; }
[ "$after" = "$clean" ] || { echo "the tondo of $older built after HEAD is not its build in a fresh target"; exit 1; }
