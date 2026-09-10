#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-conformance-coordination-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

scripts/stdlib-conformance-coordination-check.sh

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "stdlib conformance coordination: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.owners[0].rows = .owners[0].rows[1:]' testing/stdlib-conformance-coordination.json \
    > "$tmp_dir/missing-row.json"
expect_failure missing-row env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/missing-row.json" \
    scripts/stdlib-conformance-coordination-check.sh

jq '.owners[0].reason = "stale" | .owners[0].rows[0].reason = "stale"' testing/stdlib-conformance-coordination.json \
    > "$tmp_dir/missing-reason.json"
expect_failure missing-reason env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/missing-reason.json" \
    scripts/stdlib-conformance-coordination-check.sh

jq '.owners[0].status = "verified"' testing/stdlib-conformance-coordination.json \
    > "$tmp_dir/overclaim-owner.json"
expect_failure overclaim-owner env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/overclaim-owner.json" \
    scripts/stdlib-conformance-coordination-check.sh

jq '.promotion.next_coordination = "STD-CONF-001"' testing/stdlib-conformance-coordination.json \
    > "$tmp_dir/stale-next.json"
expect_failure stale-next env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/stale-next.json" \
    scripts/stdlib-conformance-coordination-check.sh

jq '.owners[0].evidence.refs = []' testing/stdlib-conformance-coordination.json \
    > "$tmp_dir/missing-ref.json"
expect_failure missing-ref env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/missing-ref.json" \
    scripts/stdlib-conformance-coordination-check.sh

jq -e --slurpfile evidence testing/stdlib-owner-evidence.json '
  . as $root
  | $root.status == "planned"
  and $root.promotion.status == "pending"
  and $root.summary.verified_rows + $root.summary.partial_rows + $root.summary.pending_rows == $root.summary.rows
  and all($root.owners[]; . as $owner |
    if (.id | IN("std.meta", "std.reflect")) then
      .status == (first($evidence[0].owners[] | select(.id == $owner.id)).cells.CONF.status)
    else .status != "verified" end)
  and any($root.owners[]; .id == "std.reflect"
    and (.evidence.cases | index("reflect-public")) != null)
  and any($root.owners[]; .id == "std.async" and .status == "pending" and (.rows | length) == 12)
  and all(["std.serialization", "std.json", "std.messagepack", "std.protobuf"][];
    . as $owner_id
    | any($root.owners[]; .id == $owner_id and (.evidence.cases | length) > 0)
  )
' testing/stdlib-conformance-coordination.json >/dev/null

for mutation in \
    '(.owners[] | select(.id == "std.meta")).evidence.scope = "native-aot"' \
    '(.owners[] | select(.id == "std.reflect")).rows[0].status = "unobserved"' \
    '(.owners[] | select(.id == "std.meta")).evidence.commands = []'; do
    jq "$mutation" testing/stdlib-conformance-coordination.json > "$tmp_dir/changed-scope.json"
    expect_failure changed-public-scope env TONDO_STDLIB_CONFORMANCE_COORDINATION="$tmp_dir/changed-scope.json" \
        scripts/stdlib-conformance-coordination-check.sh
done

echo "stdlib conformance coordination tests: OK"
