#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

evidence_dir="${TONDO_STDLIB_EVIDENCE_DIR:-target/reliability/evidence}"
mkdir -p "$evidence_dir"
samples_file="$(mktemp "$evidence_dir/stdlib-performance-samples.XXXXXX")"
trap 'rm -f "$samples_file"' EXIT

for process in 1 2 3; do
    cargo run -p tondo-stdlib --example stdlib_performance_probe --locked --quiet \
        >> "$samples_file"
done

revision="$(git rev-parse HEAD)"
cpu="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || uname -m)"
jq -Rn \
    --arg revision "$revision" \
    --arg cpu "$cpu" \
    --arg target "$(uname -m)-$(uname -s | tr '[:upper:]' '[:lower:]')" \
    --arg rustc "$(rustc --version)" \
    --arg backend "portable-scalar" \
    '
      [inputs | split("\t") | select(length == 2) | {module: .[0], nanos: (.[1] | tonumber)}]
      | group_by(.module)
      | map(. as $rows
          | ($rows | map(.nanos) | sort) as $samples
          | ($samples | length) as $n
          | {module: $rows[0].module,
             samples: $samples,
             median_ns: $samples[(($n * 0.50 | ceil) - 1)],
             p95_ns: $samples[(($n * 0.95 | ceil) - 1)],
             p99_ns: $samples[(($n * 0.99 | ceil) - 1)],
             sample_count: $n})
      | {format:"tondo-stdlib-performance-report/1",revision:$revision,cpu:$cpu,target:$target,rustc:$rustc,backend:$backend,independent_processes:3,measurements:.}
    ' < "$samples_file" > "$evidence_dir/stdlib-performance-report.json"

jq -e '(.measurements | length) == 5 and all(.measurements[]; .sample_count == 27)' \
    "$evidence_dir/stdlib-performance-report.json" >/dev/null
echo "stdlib performance report: OK"
