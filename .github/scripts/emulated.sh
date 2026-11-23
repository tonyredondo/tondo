#!/bin/sh
# Runs, under qemu's emulation, the tests built for another architecture
# that target/emulated lists, each from its package's directory as cargo
# runs it. The cases linked statically and as debug builds run natively,
# in the jobs of each architecture. The tests run as `$OWNER` (uid:gid),
# the owner of the checkout, with what they use installed: a C compiler,
# git, curl and kill.
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq gcc git curl procps > /dev/null || exit 1
exec setpriv --reuid="${OWNER%:*}" --regid="${OWNER#*:}" --clear-groups env HOME=/tmp sh -c '
# A process and every process under it.
tree() {
  echo "$1"
  for child in $(ps -o pid= --ppid "$1" 2>/dev/null); do
    tree "$child"
  done
}
# The CPU time of a process and of every process under it, user and system,
# with what the children it collected gave it (utime, stime, cutime,
# cstime). Any change is life, a fall too, as a child that ends takes its
# own away until its parent collects it; a deadlock of qemu, every thread in
# a futex of its data, spends none.
cpu() {
  total=0
  for p in $(tree "$1"); do
    stat=$(cat "/proc/$p/stat" 2>/dev/null) || continue
    set -- ${stat##*") "}
    total=$((total + ${12} + ${13} + ${14} + ${15}))
  done
  echo $total
}
# What a test binary was doing when it is stopped: the system call each of
# its threads waits in, read by this shell, its parent, as ptrace allows,
# and the processes, as it often waits on one it started.
said() {
  echo "$2"
  for thread in /proc/$1/task/*; do
    read -r name < "$thread/comm"
    read -r call < "$thread/syscall"
    echo "  $name: $call"
  done
  ps -eo pid,ppid,etimes,stat,wchan:20,args | cut -c1-200
}
# A test binary that spends no CPU for ten minutes has hung: it fails, with
# what it was doing. Slow is not hung: the cases under emulation build for
# minutes at a time, and their count grows. When the job is about to reach
# its limit, `$EMULATED_DEADLINE` (seconds since the epoch), whatever runs
# says so in the same way and the run ends red, rather than be cut with
# nothing said.
run() {
  "$@" &
  pid=$! began=$(date +%s) quiet=0
  last=$(cpu $pid)
  while kill -0 $pid 2>/dev/null; do
    now=$(date +%s)
    if [ -n "$EMULATED_DEADLINE" ] && [ "$now" -ge "$EMULATED_DEADLINE" ]; then
      said $pid "the job ends: $* ran $(( (now - began) / 60 )) min"
      kill -9 $pid
      return 3
    fi
    sleep 10
    spent=$(cpu $pid)
    if [ "$spent" = "$last" ]; then
      quiet=$((quiet + 10))
    else
      quiet=0 last=$spent
    fi
    if [ $quiet -ge 600 ]; then
      said $pid "hung: $* spent no CPU in ten minutes, $(( (now - began) / 60 )) min after it started"
      kill -9 $pid
      return 2
    fi
  done
  wait $pid
}
status=0
while read -r dir test; do
  echo "== $test from $dir, $(date -u +%H:%M:%S)"
  (cd "$dir" && run "$test")
  code=$?
  # qemu 11 can deadlock in its own locks among threads that start processes:
  # every thread in a futex of its data, and children no one collects. A
  # hang of the tests hangs again; what the first one said stays in the log.
  if [ $code = 2 ]; then
    echo "hung once, which qemu does by itself: once more"
    (cd "$dir" && run "$test")
    code=$?
  fi
  if [ $code = 3 ]; then
    exit 1
  fi
  [ $code = 0 ] || status=1
done < target/emulated
exit $status
'
