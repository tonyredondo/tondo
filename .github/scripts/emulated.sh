#!/bin/sh
# Runs, under qemu's emulation, the tests built for another architecture
# that target/emulated lists, each from its package's directory as cargo
# runs it; then the cases linked statically and as debug builds. The
# tests run as `$OWNER` (uid:gid), the owner of the checkout, with what
# they use installed: a C compiler, git, curl and kill.
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq gcc git curl procps > /dev/null || exit 1
exec setpriv --reuid="${OWNER%:*}" --regid="${OWNER#*:}" --clear-groups env HOME=/tmp sh -c '
# A test binary that has not ended in 15 minutes has hung: it fails,
# saying the system call each of its threads waits in.
run() {
  "$@" &
  pid=$! waited=0
  while kill -0 $pid 2>/dev/null; do
    if [ $waited -ge 900 ]; then
      echo "hung: $*"
      # Read by this shell, its parent, as ptrace allows.
      for thread in /proc/$pid/task/*; do
        read -r name < "$thread/comm"
        read -r call < "$thread/syscall"
        echo "  $name: $call"
      done
      kill -9 $pid
      return 1
    fi
    sleep 1
    waited=$((waited + 1))
  done
  wait $pid
}
status=0
while read -r dir test; do
  (cd "$dir" && run "$test") || status=1
done < target/emulated
cases=$(grep "/cases-" target/emulated | cut -d" " -f2)
(cd crates/tondo && TONDO_CASES_STATIC=1 run "$cases" cases && TONDO_CASES_DEBUG=1 run "$cases") || status=1
exit $status
'
