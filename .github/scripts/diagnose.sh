#!/bin/sh
# What a run that failed left in a directory of dumps, by Tondo's own tools: the
# failures grouped and a page of them, in the summary of the run. Nothing here
# fails the run: it is already failing.
dir=$1
tondo=${TONDO:-target/debug/tondo}
[ -d "$dir" ] && [ -x "$tondo" ] || exit 0
{
  echo '## What Tondo says of what hung'
  echo '```'
  "$tondo" dump group "$dir"
  for dump in "$dir"/*.dump.json; do
    [ -f "$dump" ] || continue
    echo "--- $dump"
    "$tondo" dump read "$dump" why
    "$tondo" dump read "$dump" tasks
    "$tondo" dump read "$dump" requests
  done
  echo '```'
} >> "${GITHUB_STEP_SUMMARY:-/dev/stdout}" 2>&1
"$tondo" dump serve "$dir" --output "$dir/failures.html"
exit 0
