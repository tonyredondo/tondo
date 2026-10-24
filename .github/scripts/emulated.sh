#!/bin/sh
# Runs, under qemu's emulation, the tests built for another architecture
# that target/emulated lists, each from its package's directory as cargo
# runs it; then the cases linked statically and as debug builds. The
# tests run as `$OWNER` (uid:gid), the owner of the checkout, with what
# they use installed: a C compiler, git, curl and kill.
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq gcc git curl procps > /dev/null || exit 1
exec setpriv --reuid="${OWNER%:*}" --regid="${OWNER#*:}" --clear-groups env HOME=/tmp sh -c '
status=0
while read -r dir test; do
  (cd "$dir" && "$test") || status=1
done < target/emulated
cases=$(grep "/cases-" target/emulated | cut -d" " -f2)
(cd crates/tondo && TONDO_CASES_STATIC=1 "$cases" cases && TONDO_CASES_DEBUG=1 "$cases") || status=1
exit $status
'
