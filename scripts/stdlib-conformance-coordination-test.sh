#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "$BASH_SOURCE")/.." && pwd)"
cd "$root"

tmp_root="${TMPDIR:-/tmp}"
tmp_dir="$(mktemp -d "$tmp_root/tondo-stdlib-conformance-coordination-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

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

jq '.owners[0].rows[0].reason = ""' testing/stdlib-conformance-coordination.json \
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

jq -e '
  . as $root
  | $root.summary == {
    owners: 22,
    rows: 374,
    public_signatures: 209,
    requirements: 165,
    verified_rows: 0,
    partial_rows: 373,
    pending_rows: 1,
    owner_verified: 0,
    owner_partial: 21,
    owner_pending: 1
  }
  and any($root.owners[]; .id == "std.async" and .status == "pending")
  and all(["std.serialization", "std.json", "std.messagepack", "std.protobuf"][];
    . as $owner_id
    | any($root.owners[]; .id == $owner_id and (.evidence.cases | length) > 0)
  )
' testing/stdlib-conformance-coordination.json >/dev/null

echo "stdlib conformance coordination tests: OK"
