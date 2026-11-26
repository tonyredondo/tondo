#!/bin/sh
# `tondo compiler <revision>` gives each revision its own tondo (decision
# 267): an older revision is built first in a fresh cache, then HEAD, then
# that revision again, in the same target after a newer one. The second
# build must be the first, byte for byte, and not HEAD's. One cache keeps
# every path the same, which a release build of tondo, reproducible, needs
# to give the same bytes.
#
# The older revision is the parent of the last commit that changed what a
# release build reads. Such a commit can still leave the executable as it
# was (a test inside a source file, a comment): when the first two builds
# are the same bytes, that pair cannot judge, and the revision before the
# commit that changed it is taken, in a fresh cache, three times at most.
# Equal bytes mean the same tondo there and not a stale build of HEAD,
# because `tondo compiler` refuses a build that cargo found already made
# for another revision (8da56625).
#
#   .github/scripts/compiler-identity.sh <tondo>
set -eu
tondo=$1
head=$(git rev-parse HEAD)
# The parent of the last commit up to $1 that changed what a release build
# of tondo reads: its sources and those it embeds, its dependencies and its
# toolchain; not the tests.
before_change() {
    last=$(git log -1 --format=%H "$1" -- crates vendor conformance Cargo.toml Cargo.lock \
        rust-toolchain.toml ':(exclude)crates/tondo/tests')
    [ -n "$last" ] || return 1
    git rev-parse -q --verify "$last~1^{commit}"
}
built() {
    "$tondo" compiler "$1" > "$XDG_CACHE_HOME/given" || exit 1
    path=$(tail -1 "$XDG_CACHE_HOME/given")
    [ -f "$path" ] || { echo "tondo compiler $1 gave no tondo: $path" >&2; exit 1; }
    sha256sum "$path" | cut -d' ' -f1
}
from=$head
for try in 1 2 3; do
    older=$(before_change "$from") || {
        echo "no revision before the last that changed tondo's sources: deepen the checkout"
        exit 1
    }
    XDG_CACHE_HOME=$(mktemp -d)
    export XDG_CACHE_HOME
    clean=$(built "$older")
    rm -r "$XDG_CACHE_HOME/tondo/compiler/$older"
    newer=$(built "$head")
    [ "$clean" = "$newer" ] || break
    echo "$older builds HEAD's tondo ($clean): taking the revision before its change"
    [ "$try" -lt 3 ] || {
        echo "none of three revisions before HEAD's last changes builds another tondo: nothing to judge with"
        exit 1
    }
    from=$older
done
after=$(built "$older")
echo "$older first: $clean; HEAD $head: $newer; $older after HEAD: $after"
[ "$after" != "$newer" ] || { echo "the tondo of $older built after HEAD is HEAD's"; exit 1; }
[ "$after" = "$clean" ] || { echo "the tondo of $older built after HEAD is not its build in a fresh target"; exit 1; }
