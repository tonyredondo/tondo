#!/bin/sh
# `tondo test --bench --series` (testing 10.4) over the last change to what
# tondo is built from: the revision before it and HEAD, each built by
# `tondo compiler`, bench/tondo's arrays measured with each. The series
# must pass and name each revision's sha256, and a second run must take
# every measure from the cache, as nothing that decides them changed.
#
#   .github/scripts/series.sh <tondo>
set -eu
tondo=$1
# What a release build of tondo reads, as driver.rs's TONDO_SOURCES says.
last=$(git log -1 --format=%H HEAD -- crates vendor conformance Cargo.toml Cargo.lock \
    rust-toolchain.toml ':(exclude)crates/tondo/tests')
if [ -z "$last" ] || ! older=$(git rev-parse -q --verify "$last~1^{commit}"); then
    echo "no revision before the last that changed tondo's sources: deepen the checkout"
    exit 1
fi
out=$(mktemp -d)
series() { "$tondo" test bench/tondo --bench --bench-time 0.05 --filter arrays --series "$older..HEAD" --json > "$1"; }
series "$out/first.json"
jq -e '.format == "tondo-series-0.1/1" and (.series.revisions | length >= 2)
    and all(.series.revisions[]; .sha256 | test("^[0-9a-f]{64}$"))' "$out/first.json" > /dev/null ||
    { echo "the series' report lacks its revisions or their sha256s"; cat "$out/first.json"; exit 1; }
series "$out/second.json"
jq -e 'all(.series.revisions[].measured[]; .now == false)' "$out/second.json" > /dev/null ||
    { echo "the second series measured again what the first kept"; cat "$out/second.json"; exit 1; }
echo "series $older..HEAD: $(jq -c '[.series.revisions[] | {revision: .revision[:8], sha256: .sha256[:16]}]' "$out/first.json")"
