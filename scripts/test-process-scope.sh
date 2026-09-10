#!/usr/bin/env bash
# Run repository checks inside the caller's explicit delegated OS scope.
# Production `tondo test` still receives --process-cgroup on its command line.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: test-process-scope.sh <check-command> [arguments...]" >&2
  exit 2
fi

if [ "$(uname -s)" = Linux ]; then
  IFS=: read -r tondo_group_id tondo_group_controllers tondo_group_path < /proc/self/cgroup
  if [ "$tondo_group_id" != 0 ] || [ -n "$tondo_group_controllers" ] || [[ "$tondo_group_path" != /* ]]; then
    echo "process checks require a delegated cgroup-v2 scope" >&2
    exit 3
  fi
  tondo_process_root="/sys/fs/cgroup$tondo_group_path"
  if [ ! -w "$tondo_process_root" ] || [ ! -w "$tondo_process_root/cgroup.procs" ]; then
    echo "process checks require a writable delegated scope; see docs/contracts/test-interrupt.md" >&2
    exit 3
  fi
  export TONDO_TEST_PROCESS_CGROUP="$tondo_process_root"
fi
exec "$@"
