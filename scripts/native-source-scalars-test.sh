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

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-aggregate-calls.to > "$tmp/calls-probe.json"
comparison=()
if [[ -n "${TONDO_LLVM_LLC:-}" ]]; then
    comparison=(--llvm "$TONDO_LLVM_LLC")
fi
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/calls-probe.json" --output "$tmp/calls-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-generic-calls.to > "$tmp/generics-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-generic-calls.to > "$tmp/generics-repeated.json"
cmp "$tmp/generics-probe.json" "$tmp/generics-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/generics-probe.json" --output "$tmp/generics-report.json"

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
calls = json.loads((root / 'calls-report.json').read_text())
assert calls['format'] == report['format'] and calls['backend'] == 'cranelift'
assert calls['boundary'] == report['boundary']
assert calls['n1_claim'] is False and calls['production_runtime_linked'] is False
cases = calls['observations']
assert len(cases) == 16 and len({case['function_ordinal'] for case in cases}) == 11
assert sum(case['native_status'] == 'returned' for case in cases) == 15
assert sum(case['native_status'] == 'trapped' for case in cases) == 1
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in calls:
    comparison = calls['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
probe = json.loads((root / 'probe.json').read_text())
generics = json.loads((root / 'generics-report.json').read_text())
assert generics['format'] == report['format'] and generics['backend'] == 'cranelift'
assert generics['boundary'] == report['boundary']
assert generics['n1_claim'] is False and generics['production_runtime_linked'] is False
cases = generics['observations']
assert len(cases) == 38 and len({case['function_ordinal'] for case in cases}) == 18
assert sum(case['native_status'] == 'trapped' for case in cases) == 1
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in generics:
    comparison = generics['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
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
probe = json.loads((root / 'calls-probe.json').read_text())
for name in ['result-width', 'aliased-results', 'scalar-result-protocol', 'missing-successor']:
    candidate = copy.deepcopy(probe)
    block = next(block for function in candidate['fixtures'][0]['mir']['backend']['functions']
                 for block in function['blocks'] if 'CallAggregate' in block['terminator'])
    call = block['terminator']['CallAggregate']
    if name == 'result-width':
        call['destinations'].pop()
    elif name == 'aliased-results':
        call['destinations'][1] = call['destinations'][0]
    elif name == 'missing-successor':
        call['target'] = None
    else:
        block['terminator'] = {'Invoke': {
            'operation': {'Call': {'function': call['function'], 'arguments': call['arguments']}},
            'destination': call['destinations'][0], 'target': call['target'],
        }}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
probe = json.loads((root / 'generics-probe.json').read_text())
for name in ['missing-template', 'template-admitted', 'duplicate-instance', 'incomplete-instance', 'call-template']:
    candidate = copy.deepcopy(probe)
    functions = candidate['fixtures'][0]['mir']['backend']['functions']
    template = next(f for f in functions if 'Template' in f.get('generics', {}))
    instances = [f for f in functions if 'Instance' in f.get('generics', {})]
    instance = instances[0]['generics']['Instance']
    if name == 'missing-template':
        instance['template'] = 999999
    elif name == 'template-admitted':
        template['supported'] = True
    elif name == 'duplicate-instance':
        instances[1]['generics'] = copy.deepcopy(instances[0]['generics'])
    elif name == 'incomplete-instance':
        instance['arguments'] = []
    else:
        call = next(block['terminator']['Invoke']['operation']['Call']
                    for function in functions if function['supported']
                    for block in function['blocks']
                    if 'Call' in block['terminator'].get('Invoke', {}).get('operation', {}))
        call['function'] = template['ordinal']
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
PY

for candidate in source-drift unsupported missing-observation oracle-drift empty \
    result-width aliased-results scalar-result-protocol missing-successor \
    missing-template template-admitted duplicate-instance incomplete-instance call-template; do
    if "$adapter" "${args[@]}" --probe "$tmp/$candidate.json" --output "$tmp/rejected.json" \
        > "$tmp/$candidate.log" 2>&1; then
        echo "native source scalars: $candidate unexpectedly passed" >&2
        exit 1
    fi
    [[ ! -e "$tmp/rejected.json" ]] || { echo "native source scalars: partial report escaped" >&2; exit 1; }
done
cp "$tmp/report.json" "$target_dir/reliability/evidence/native-source-scalars.json"
cp "$tmp/records-report.json" "$target_dir/reliability/evidence/native-source-records.json"
cp "$tmp/calls-report.json" "$target_dir/reliability/evidence/native-source-calls.json"
cp "$tmp/generics-report.json" "$target_dir/reliability/evidence/native-source-generics.json"
echo "native source scalars: OK (99 Cranelift cases, 5 arithmetic traps, 14 rejected evidence changes)"
if [[ ${#comparison[@]} -gt 0 ]]; then
    echo "native aggregate and generic calls: LLVM comparison OK (54 cases, 2 arithmetic traps)"
fi
