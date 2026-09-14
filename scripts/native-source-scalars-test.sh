#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
[[ "$target_dir" = /* ]] || target_dir="$root/$target_dir"
cc="${TONDO_NATIVE_CC:-/usr/bin/cc}"
[[ "$cc" = /* && -x "$cc" ]] || { echo "native source scalars: explicit C driver required" >&2; exit 1; }
[[ "$(rustc -vV | sed -n 's/^host: //p')" == x86_64-unknown-linux-gnu ]] \
    || { echo "native source scalars: x86_64 GNU Linux host required" >&2; exit 1; }
mkdir -p "$root/.tmp" "$target_dir/reliability/evidence"
tmp="$(mktemp -d "$root/.tmp/native-source-scalars.XXXXXX")"
trap 'rm -rf -- "$tmp"' EXIT
ulimit -c 0

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-local-tuples.to > "$tmp/probe.json"
CARGO_TARGET_DIR="$target_dir/native-evaluation" cargo build \
    --manifest-path tools/native-evaluation/Cargo.toml --bin tondo-native-evaluation --locked --quiet
adapter="$target_dir/native-evaluation/debug/tondo-native-evaluation"
args=(--source-scalars --target x86_64-unknown-linux-gnu --cc "$cc" --temp-dir "$tmp/native")
"$adapter" "${args[@]}" --probe "$tmp/probe.json" --output "$tmp/report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-local-records.to > "$tmp/records-probe.json"
"$adapter" "${args[@]}" --probe "$tmp/records-probe.json" --output "$tmp/records-report.json"

python3 - "$tmp" <<'PY'
import copy
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
report = json.loads((root / 'report.json').read_text())
assert report['format'] == 'tondo-native-source-scalars/1'
assert report['backend'] == 'cranelift'
assert report['boundary'] == 'source-driven-runtime-free-scalar-functions'
assert report['n1_claim'] is False and report['production_runtime_linked'] is False
cases = report['observations']
assert len(cases) == 24 and len({case['function_ordinal'] for case in cases}) == 9
assert sum(case['native_status'] == 'returned' for case in cases) == 22
assert sum(case['native_status'] == 'trapped' for case in cases) == 2
assert all(case['native_result'] == case['vm_result'] for case in cases)
records = json.loads((root / 'records-report.json').read_text())
assert records['format'] == report['format'] and records['backend'] == 'cranelift'
assert records['boundary'] == report['boundary']
assert records['n1_claim'] is False and records['production_runtime_linked'] is False
cases = records['observations']
assert len(cases) == 21 and len({case['function_ordinal'] for case in cases}) == 11
assert sum(case['native_status'] == 'returned' for case in cases) == 20
assert sum(case['native_status'] == 'trapped' for case in cases) == 1
assert all(case['native_result'] == case['vm_result'] for case in cases)
probe = json.loads((root / 'probe.json').read_text())
for name in ['source-drift', 'unsupported', 'missing-observation', 'oracle-drift', 'empty']:
    candidate = copy.deepcopy(probe)
    fixture = candidate['fixtures'][0]
    if name == 'source-drift':
        fixture['fixture_sha256'] = 'sha256:' + '0' * 64
    elif name == 'unsupported':
        fixture['mir']['backend']['functions'][0]['supported'] = False
    elif name == 'missing-observation':
        fixture['vm_scalar'] = []
    elif name == 'oracle-drift':
        observation = next(row for row in fixture['vm_scalar'] if row['status'] == 'returned')
        observation['result'] += 1
    else:
        candidate['fixtures'] = []
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
PY

for candidate in source-drift unsupported missing-observation oracle-drift empty; do
    if "$adapter" "${args[@]}" --probe "$tmp/$candidate.json" --output "$tmp/rejected.json" \
        > "$tmp/$candidate.log" 2>&1; then
        echo "native source scalars: $candidate unexpectedly passed" >&2
        exit 1
    fi
    [[ ! -e "$tmp/rejected.json" ]] || { echo "native source scalars: partial report escaped" >&2; exit 1; }
done
cp "$tmp/report.json" "$target_dir/reliability/evidence/native-source-scalars.json"
cp "$tmp/records-report.json" "$target_dir/reliability/evidence/native-source-records.json"
echo "native source scalars: OK (45 observed cases, 3 arithmetic traps, 5 rejected evidence changes)"
