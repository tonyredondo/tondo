#!/bin/sh
# Runs what `cargo test --workspace` runs, its test binaries two at a time
# instead of one after another: tests.sh [<at once>]. Each runs from its
# package's directory, as cargo runs it, the longest first; each says when
# it starts, so that one that hangs shows, and its output comes whole once
# it ends. A binary is one process, as under cargo, which the tests that
# take ports and directories by process rely on.
set -u
at_once=${1:-2}
work=$(mktemp -d)
cargo test --workspace --no-run --message-format=json > "$work/built" || exit 1
jq -r 'select(.profile.test == true and .executable != null)
    | (.manifest_path | rtrimstr("Cargo.toml")) + " " + .executable' "$work/built" > "$work/all" || exit 1
# The longest of x86's: the cases, then the tests of `tondo test` and of
# the tools that observe a program.
{
    grep -E '/(cases|testing|observe)-[0-9a-f]+$' "$work/all"
    grep -vE '/(cases|testing|observe)-[0-9a-f]+$' "$work/all"
} > "$work/order"
export work
tr '\n' '\0' < "$work/order" | xargs -0 -n 1 -P "$at_once" sh -c '
    dir=${1% *} test=${1##* } name=$(basename "${1##* }")
    echo "started $name"
    start=$(date +%s)
    (cd "$dir" && "$test") > "$work/$name.log" 2>&1
    code=$?
    echo $code > "$work/$name.code"
    until mkdir "$work/lock" 2>/dev/null; do sleep 1; done
    echo "::group::$name: exit $code in $(($(date +%s) - start)) s"
    cat "$work/$name.log"
    echo "::endgroup::"
    rmdir "$work/lock"
' tests
status=0
while read -r line; do
    name=$(basename "${line##* }")
    code=$(cat "$work/$name.code" 2>/dev/null || echo missing)
    if [ "$code" != 0 ]; then
        echo "failed: $name ($code)"
        grep -E '^test .* FAILED$|panicked at' "$work/$name.log" | head -20
        status=1
    fi
done < "$work/order"
# The documentation's examples, none today, as cargo would run them.
cargo test --workspace --doc || status=1
rm -rf "$work"
exit $status
