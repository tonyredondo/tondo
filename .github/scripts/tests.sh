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
cargo test --workspace --no-run --message-format=json-render-diagnostics > "$work/built" || exit 1
jq -r 'select(.profile.test == true and .executable != null)
    | (.manifest_path | rtrimstr("Cargo.toml")) + " " + .executable' "$work/built" > "$work/all" || exit 1
# The longest of x86's: the cases, then the tests of `tondo test` and of
# the tools that observe a program.
{
    grep -E '/(cases|testing|observe)-[0-9a-f]+$' "$work/all"
    grep -vE '/(cases|testing|observe)-[0-9a-f]+$' "$work/all"
} > "$work/order"
# On Linux a watchdog ends a binary, in a process group of its own, that
# runs past half an hour or holds past its share of the machine's memory,
# with what each of its processes held and a QUIT first, at which a Tondo
# program says where each task is: a runaway test then fails with its log,
# where otherwise it took the runner down with it, log and all.
share=
if [ -r /proc/meminfo ] && command -v setsid > /dev/null; then
    share=$(awk -v at_once="$at_once" '/MemTotal/ { print int($2 * 0.6 / at_once) }' /proc/meminfo)
fi
export work share
tr '\n' '\0' < "$work/order" | xargs -0 -n 1 -P "$at_once" sh -c '
    dir=${1% *} test=${1##* } name=$(basename "${1##* }")
    echo "started $name"
    start=$(date +%s)
    if [ -n "$share" ]; then
        (cd "$dir" && exec setsid "$test") > "$work/$name.log" 2>&1 &
        pid=$!
        page=$(($(getconf PAGESIZE) / 1024))
        # The processes of its group: the id, parent, state and KiB held of
        # each, and with "all", what it waits for in the kernel and its
        # command. From /proc, as BusyBox ps selects no group, and with no
        # process of its own for each one.
        group() {
            all=${1:-}
            for proc in /proc/[0-9]*; do
                read -r line 2> /dev/null < "$proc/stat" || continue
                set -- ${line##*) }
                [ "$3" = "$pid" ] || continue
                state=$1 parent=$2
                read -r _ pages _ 2> /dev/null < "$proc/statm" || continue
                if [ -n "$all" ]; then
                    # It has no newline, which read takes as a failure.
                    wchan=
                    read -r wchan 2> /dev/null < "$proc/wchan"
                    echo "${proc#/proc/} $parent $state $((pages * page)) ${wchan:-?} $(tr "\0" " " < "$proc/cmdline" 2> /dev/null)"
                else
                    echo "$((pages * page))"
                fi
            done
        }
        # Every process that descends from the binary, or is of its group,
        # with its threads: the call each is in and what it waits for in
        # the kernel, and its open files; then the locks of the system. A
        # hang is read from this, where a stack alone says little: which
        # build waits, on what, and who holds it.
        threads() {
            family=" $pid " grew=1
            while [ -n "$grew" ]; do
                grew=
                for proc in /proc/[0-9]*; do
                    read -r line 2> /dev/null < "$proc/stat" || continue
                    set -- ${line##*) }
                    case "$family" in *" ${proc#/proc/} "*) continue ;; esac
                    case "$family" in *" $2 "*) ;; *) [ "$3" = "$pid" ] || continue ;; esac
                    family="$family${proc#/proc/} " grew=1
                done
            done
            for proc in $family; do
                echo "process $proc: $(tr "\0" " " < "/proc/$proc/cmdline" 2> /dev/null | cut -c1-300)"
                for thread in /proc/"$proc"/task/*; do
                    comm= wchan= call=
                    read -r comm 2> /dev/null < "$thread/comm"
                    read -r wchan 2> /dev/null < "$thread/wchan"
                    read -r call 2> /dev/null < "$thread/syscall"
                    echo "  thread ${thread##*/} $comm: waits in ${wchan:-?}, call ${call:-?}"
                done
                for fd in /proc/"$proc"/fd/*; do
                    echo "  fd ${fd##*/}: $(readlink "$fd" 2> /dev/null)"
                done
            done
            echo "locks:"
            cat /proc/locks 2> /dev/null
        }
        while kill -0 "$pid" 2> /dev/null; do
            held=0
            for kib in $(group); do held=$((held + kib)); done
            if [ "$held" -gt "$share" ] || [ $(($(date +%s) - start)) -gt 1800 ]; then
                {
                    echo "watchdog: $name holds $held KiB after $(($(date +%s) - start)) s, past $share KiB or 1800 s:"
                    echo "pid parent state KiB wchan command"
                    group all
                    threads
                } >> "$work/$name.log" 2>&1
                # With no "--", which dash takes for a process, and then
                # signals none.
                kill -QUIT "-$pid" 2> /dev/null
                sleep 5
                kill -KILL "-$pid" 2> /dev/null
                break
            fi
            sleep 1
        done
        wait "$pid"
    else
        (cd "$dir" && "$test") > "$work/$name.log" 2>&1
    fi
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
