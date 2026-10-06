#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_UUID_PERF_CONTRACT:-$root/testing/stdlib-uuid-performance.json}"
target_dir="${CARGO_TARGET_DIR:-target}"
if [[ "$target_dir" == /* ]]; then
    default_evidence="$target_dir/reliability/evidence"
else
    default_evidence="$root/$target_dir/reliability/evidence"
fi
evidence="${TONDO_STDLIB_UUID_PERF_EVIDENCE_DIR:-$default_evidence}"
die() { echo "std.uuid performance: $*" >&2; exit 1; }

scripts/stdlib-uuid-performance-check.sh >/dev/null || die "contract check failed"
capture="clean-source"
if [[ "${TONDO_STDLIB_UUID_PERF_ALLOW_DIRTY:-0}" == 1 ]]; then
    capture="development"
else
    [[ -z "$(git status --porcelain)" ]] || die "workspace must be clean"
fi
for variable in RUSTC RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS \
    CARGO_BUILD_RUSTC CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER; do
    [[ ! -v "$variable" ]] || die "unsupported compiler override: $variable"
done
[[ -z "$(compgen -A variable CARGO_PROFILE_ || true)" ]] || die "unsupported profile override"
while IFS= read -r variable; do
    [[ "$variable" != *_RUSTFLAGS ]] || die "unsupported target flag override: $variable"
done < <(compgen -A variable CARGO_TARGET_ || true)
target="$(rustc -vV | awk '/^host:/ { print $2 }')"
[[ "$target" == "$(jq -r '.target' "$contract")" ]] || die "rustc host differs from pinned target"
revision="$(git rev-parse HEAD)"

mkdir -p "$root/.tmp"
tmp="$(mktemp -d "$root/.tmp/tondo-uuid-performance.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
samples="$tmp/samples.jsonl"
: > "$samples"
cargo run -q -p tondo-reliability --locked -- quality provenance --root . > "$tmp/before.json"
tree="$(jq -r '.tree_sha256' "$tmp/before.json")"
timeout 120s cargo test -q -p tondo-reliability --test uuid_models --locked > "$tmp/model.log" 2>&1 \
    || { cat "$tmp/model.log" >&2; die "independent oracle failed"; }
timeout 180s cargo test -q -p tondo-compiler --lib process_host::uuid::performance --locked > "$tmp/fixtures.log" 2>&1 \
    || { cat "$tmp/fixtures.log" >&2; die "fixture, lifecycle or batch oracle failed"; }
for process in 1 2 3; do
    log="$tmp/process-$process.log"
    env TONDO_UUID_PERF_RUN=1 TONDO_UUID_PERF_PROCESS="$process" timeout 60s \
        cargo test -q -p tondo-compiler --lib --locked process_host::uuid::performance::uuid_performance_probe \
        -- --exact --nocapture > "$log" 2>&1 \
        || { cat "$log" >&2; die "probe failed in independent process $process"; }
    count="$(grep -c $'^TONDO_UUID_PERF\t' "$log" || true)"
    [[ "$count" == 198 ]] || die "process $process emitted $count samples instead of 198"
    grep $'^TONDO_UUID_PERF\t' "$log" | cut -f2- >> "$samples"
done
cargo run -q -p tondo-reliability --locked -- quality provenance --root . > "$tmp/after.json"
cmp -s "$tmp/before.json" "$tmp/after.json" || die "source, flags or toolchain changed during capture"
[[ "$revision" == "$(git rev-parse HEAD)" ]] || die "revision changed during capture"
if [[ "$capture" == clean-source ]]; then
    [[ -z "$(git status --porcelain)" ]] || die "workspace became dirty during capture"
fi
cpu="$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//' || uname -m)"
report="$evidence/stdlib-uuid-performance.json"
python3 -B scripts/stdlib_uuid_performance.py render \
    --contract "$contract" --samples "$samples" --report "$report" \
    --revision "$revision" --tree-sha256 "$tree" --target "$target" \
    --rustc "$(rustc --version)" --cargo "$(cargo --version)" \
    --rustflags="${RUSTFLAGS-}" --encoded-rustflags="${CARGO_ENCODED_RUSTFLAGS-}" \
    --incremental="${CARGO_INCREMENTAL-}" --cpu-model "$cpu" --os "$(uname -s) $(uname -r)" \
    --capture "$capture"
python3 -B scripts/stdlib_uuid_performance.py check-report \
    --contract "$contract" --report "$report" --tree-sha256 "$tree"
echo "std.uuid performance: OK (22 hosted bridge workloads; 27 retained samples each; $capture; report: $report)"
