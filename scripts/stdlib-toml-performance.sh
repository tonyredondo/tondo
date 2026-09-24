#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_TOML_PERF_CONTRACT:-$root/testing/stdlib-toml-performance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" == /* ]]; then
    default_evidence_dir="$target_dir/reliability/evidence"
else
    default_evidence_dir="$root/$target_dir/reliability/evidence"
fi
evidence_dir="${TONDO_STDLIB_TOML_PERF_EVIDENCE_DIR:-$default_evidence_dir}"

die() { echo "std.toml performance: $*" >&2; exit 1; }

scripts/stdlib-toml-performance-check.sh >/dev/null || die "contract check failed"
if [[ "${TONDO_STDLIB_TOML_PERF_ALLOW_DIRTY:-0}" != 1 ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi
target="$(rustc -vV | awk '/^host:/ { print $2 }')"
[[ "$target" == "$(jq -r '.target' "$contract")" ]] || die "rustc host differs from pinned target"

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-stdlib-toml-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.jsonl"
: > "$samples"

env CARGO_TARGET_DIR="$target_dir" timeout 120s \
    cargo test -p tondo-reliability --test toml_models --locked \
    > "$tmp/model.log" 2>&1 || { cat "$tmp/model.log" >&2; die "independent oracle failed"; }
env CARGO_TARGET_DIR="$target_dir" timeout 60s \
    cargo test -p tondo-stdlib --test toml_performance --locked fixture_oracles_are_exact \
    -- --exact > "$tmp/fixtures.log" 2>&1 \
    || { cat "$tmp/fixtures.log" >&2; die "fixture oracle failed"; }

for process in 1 2 3; do
    log="$tmp/process-$process.log"
    env CARGO_TARGET_DIR="$target_dir" TONDO_TOML_PERF_RUN=1 timeout 60s \
        cargo test -p tondo-stdlib --test toml_performance --locked toml_performance_probe \
        -- --exact --nocapture > "$log" 2>&1 \
        || { cat "$log" >&2; die "probe failed in independent process $process"; }
    count="$(grep -c '^TONDO_TOML_PERF' "$log" || true)"
    [[ "$count" == 117 ]] || die "process $process emitted $count samples instead of 117"
    grep '^TONDO_TOML_PERF' "$log" | cut -f2- >> "$samples"
done

revision="$(git rev-parse HEAD)"
cpu="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || uname -m)"
[[ -n "$cpu" ]] || cpu="unavailable"
os="$(uname -s) $(uname -r)"
report="$evidence_dir/stdlib-toml-performance.json"
python3 -B scripts/stdlib_toml_performance.py render \
    --contract "$contract" --samples "$samples" --report "$report" \
    --revision "$revision" --target "$target" \
    --rustc "$(rustc --version)" --cargo "$(cargo --version)" \
    --flags "${RUSTFLAGS-}" --cpu-model "$cpu" --os "$os"
python3 -B scripts/stdlib_toml_performance.py check-report \
    --contract "$contract" --report "$report"
echo "std.toml performance: OK (13 workloads; 27 samples each; report: $report)"
