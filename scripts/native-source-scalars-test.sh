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

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-aggregate-equality.to > "$tmp/equality-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-aggregate-equality.to > "$tmp/equality-repeated.json"
cmp "$tmp/equality-probe.json" "$tmp/equality-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/equality-probe.json" --output "$tmp/equality-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-unit-aggregates.to > "$tmp/units-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-unit-aggregates.to > "$tmp/units-repeated.json"
cmp "$tmp/units-probe.json" "$tmp/units-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/units-probe.json" --output "$tmp/units-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-integer-aggregates.to > "$tmp/integers-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-integer-aggregates.to > "$tmp/integers-repeated.json"
cmp "$tmp/integers-probe.json" "$tmp/integers-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/integers-probe.json" --output "$tmp/integers-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-sum-values.to > "$tmp/sums-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-sum-values.to > "$tmp/sums-repeated.json"
cmp "$tmp/sums-probe.json" "$tmp/sums-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/sums-probe.json" --output "$tmp/sums-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-enum-values.to > "$tmp/enums-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-enum-values.to > "$tmp/enums-repeated.json"
cmp "$tmp/enums-probe.json" "$tmp/enums-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/enums-probe.json" --output "$tmp/enums-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-union-values.to > "$tmp/unions-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-union-values.to > "$tmp/unions-repeated.json"
cmp "$tmp/unions-probe.json" "$tmp/unions-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/unions-probe.json" --output "$tmp/unions-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-uint64-values.to > "$tmp/uint64-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-uint64-values.to > "$tmp/uint64-repeated.json"
cmp "$tmp/uint64-probe.json" "$tmp/uint64-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/uint64-probe.json" --output "$tmp/uint64-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-float-values.to > "$tmp/floats-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-float-values.to > "$tmp/floats-repeated.json"
cmp "$tmp/floats-probe.json" "$tmp/floats-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/floats-probe.json" --output "$tmp/floats-report.json"

CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-char-values.to > "$tmp/chars-probe.json"
CARGO_TARGET_DIR="$target_dir" cargo run -p tondo-compiler --example native_mir_probe \
    --locked --quiet -- tests/native/native-aot-char-values.to > "$tmp/chars-repeated.json"
cmp "$tmp/chars-probe.json" "$tmp/chars-repeated.json"
"$adapter" "${args[@]}" "${comparison[@]}" --probe "$tmp/chars-probe.json" --output "$tmp/chars-report.json"

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
equality = json.loads((root / 'equality-report.json').read_text())
assert equality['format'] == report['format'] and equality['backend'] == 'cranelift'
assert equality['boundary'] == report['boundary']
assert equality['n1_claim'] is False and equality['production_runtime_linked'] is False
cases = equality['observations']
assert len(cases) == 36 and len({case['function_ordinal'] for case in cases}) == 16
assert sum(case['native_status'] == 'trapped' for case in cases) == 1
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in equality:
    comparison = equality['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
units = json.loads((root / 'units-report.json').read_text())
assert units['format'] == report['format'] and units['backend'] == 'cranelift'
assert units['boundary'] == report['boundary']
assert units['n1_claim'] is False and units['production_runtime_linked'] is False
cases = units['observations']
assert len(cases) == 43 and len({case['function_ordinal'] for case in cases}) == 13
assert sum(case['native_status'] == 'trapped' for case in cases) == 2
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in units:
    comparison = units['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
integers = json.loads((root / 'integers-report.json').read_text())
assert integers['format'] == report['format'] and integers['backend'] == 'cranelift'
assert integers['boundary'] == report['boundary']
assert integers['n1_claim'] is False and integers['production_runtime_linked'] is False
cases = integers['observations']
assert len(cases) == 67 and len({case['function_ordinal'] for case in cases}) == 37
assert sum(case['native_status'] == 'trapped' for case in cases) == 38
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in integers:
    comparison = integers['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
sums = json.loads((root / 'sums-report.json').read_text())
assert sums['format'] == report['format'] and sums['backend'] == 'cranelift'
assert sums['boundary'] == report['boundary']
assert sums['n1_claim'] is False and sums['production_runtime_linked'] is False
cases = sums['observations']
assert len(cases) == 71 and len({case['function_ordinal'] for case in cases}) == 26
assert sum(case['native_status'] == 'trapped' for case in cases) == 3
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in sums:
    comparison = sums['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
sum_probe = json.loads((root / 'sums-probe.json').read_text())
names = {symbol['function']: symbol['name'].split('::value::')[-1]
         for symbol in sum_probe['fixtures'][0]['mir']['backend']['debug']['symbols']}
expected = {
    'clearedOption': 1, 'earlyError': -7, 'earlyNone': 1, 'genericValues': 42,
    'integerFailures': 1, 'integerLimits': 1, 'narrowSources': 1, 'nestedCopies': 42,
    'nestedOptions': 42, 'numericErrorTags': 1, 'overlappingEquality': 1,
    'repeatedCalls': 6, 'unitValues': 1, 'variantCopies': 42,
}
for case in cases:
    name = names[case['function_ordinal']]
    if name in expected:
        assert case['arguments'] == [] and case['native_result'] == expected[name], name
    elif case['arguments']:
        value, = case['arguments']
        if name in ['conversionCase', 'projectedConversion', 'arithmeticAfterConversion']:
            result = value + (name == 'arithmeticAfterConversion') if -128 <= value <= 127 else -1000
        else:
            positive, negative = {
                'optionCase': (42, -1), 'resultCase': (42, 293), 'nestedResults': (42, 500),
                'optionInResult': (42, 0), 'resultInOption': (42, 300), 'recursion': (84, -7),
            }[name]
            result = positive if value > 0 else negative
        assert case['native_result'] == result, (name, value)
    else:
        assert name in ['convertedOverflow', 'discardedTraps', 'successStillTraps']
        assert case['native_status'] == 'trapped'
enums = json.loads((root / 'enums-report.json').read_text())
assert enums['format'] == report['format'] and enums['backend'] == 'cranelift'
assert enums['boundary'] == report['boundary']
assert enums['n1_claim'] is False and enums['production_runtime_linked'] is False
cases = enums['observations']
assert len(cases) == 38 and len({case['function_ordinal'] for case in cases}) == 28
assert sum(case['native_status'] == 'trapped' for case in cases) == 3
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in enums:
    comparison = enums['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
enum_probe = json.loads((root / 'enums-probe.json').read_text())
names = {symbol['function']: symbol['name'].split('::value::')[-1]
         for symbol in enum_probe['fixtures'][0]['mir']['backend']['debug']['symbols']}
for case in cases:
    name = names[case['function_ordinal']]
    if name in ['discardedTrap', 'overflowPayload', 'propagatedTrap']:
        assert case['arguments'] == [] and case['native_status'] == 'trapped', name
        continue
    if name == 'enumValue':
        value, = case['arguments']
        expected = value if value >= 0 else -1
    elif name == 'customErrorCase':
        value, = case['arguments']
        expected = 42 if value > 0 else 41
    else:
        assert case['arguments'] == [], name
        expected = {'positional': 293, 'narrowPayloads': 4294967167}.get(name, 42)
    assert case['native_status'] == 'returned' and case['native_result'] == expected, name
unions = json.loads((root / 'unions-report.json').read_text())
assert unions['format'] == report['format'] and unions['backend'] == 'cranelift'
assert unions['boundary'] == report['boundary']
assert unions['n1_claim'] is False and unions['production_runtime_linked'] is False
cases = unions['observations']
assert len(cases) == 50 and len({case['function_ordinal'] for case in cases}) == 40
assert sum(case['native_status'] == 'trapped' for case in cases) == 3
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in unions:
    comparison = unions['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
union_probe = json.loads((root / 'unions-probe.json').read_text())
names = {symbol['function']: symbol['name'].split('::value::')[-1]
         for symbol in union_probe['fixtures'][0]['mir']['backend']['debug']['symbols']}
for case in cases:
    name = names[case['function_ordinal']]
    if name in ['discardedTrap', 'overflowMember', 'narrowOverflow']:
        assert case['arguments'] == [] and case['native_status'] == 'trapped', name
        continue
    if name == 'unionCase':
        value, = case['arguments']
        expected = value if value > 0 else -1
    elif name == 'errorCase':
        value, = case['arguments']
        expected = 42 if value > 0 else 41 if value == 0 else -1
    else:
        assert case['arguments'] == [], name
        expected = 4294967295 if name == 'narrowMember' else 42
    assert case['native_status'] == 'returned' and case['native_result'] == expected, name
uint64 = json.loads((root / 'uint64-report.json').read_text())
assert uint64['format'] == report['format'] and uint64['backend'] == 'cranelift'
assert uint64['boundary'] == report['boundary']
assert uint64['n1_claim'] is False and uint64['production_runtime_linked'] is False
cases = uint64['observations']
assert len(cases) == 54 and len({case['function_ordinal'] for case in cases}) == 49
assert sum(case['native_status'] == 'trapped' for case in cases) == 12
assert all(case['native_result'] == case['vm_result'] for case in cases)
if 'llvm_comparison' in uint64:
    comparison = uint64['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
uint64_probe = json.loads((root / 'uint64-probe.json').read_text())
names = {symbol['function']: symbol['name'].split('::value::')[-1]
         for symbol in uint64_probe['fixtures'][0]['mir']['backend']['debug']['symbols']}
traps = {'overflowAdd', 'overflowSubtract', 'overflowMultiply', 'overflowMaximumProduct',
         'divideByZero', 'remainderByZero', 'negativeLeftShift', 'oversizedLeftShift', 'hugeLeftShift',
         'negativeRightShift', 'oversizedRightShift', 'hugeRightShift'}
assert {names[case['function_ordinal']] for case in cases if case['native_status'] == 'trapped'} == traps
for case in cases:
    name = names[case['function_ordinal']]
    if name in traps:
        assert case['arguments'] == [] and case['native_status'] == 'trapped', name
    else:
        assert case['arguments'] == [] or (name == 'dynamicRoundTrip' and len(case['arguments']) == 1)
        assert case['native_status'] == 'returned' and case['native_result'] == 42, (name, case)
floats = json.loads((root / 'floats-report.json').read_text())
assert floats['format'] == report['format'] and floats['backend'] == 'cranelift'
assert floats['boundary'] == report['boundary']
assert floats['n1_claim'] is False and floats['production_runtime_linked'] is False
cases = floats['observations']
assert len(cases) == 69 and len({case['function_ordinal'] for case in cases}) == 69
assert all(case['arguments'] == [] and case['native_status'] == 'returned'
           and case['native_result'] == case['vm_result'] == 42 for case in cases)
if 'llvm_comparison' in floats:
    comparison = floats['llvm_comparison']
    assert comparison['version']
    assert comparison['observations'] == [
        {key: case[key] for key in ['function_ordinal', 'arguments', 'native_status', 'native_result']}
        for case in cases
    ]
float_probe = json.loads((root / 'floats-probe.json').read_text())
char_probe = json.loads((root / 'chars-probe.json').read_text())
chars = json.loads((root / 'chars-report.json').read_text())
assert chars['format'] == report['format'] and chars['backend'] == 'cranelift'
assert chars['boundary'] == report['boundary']
assert chars['n1_claim'] is False and chars['production_runtime_linked'] is False
cases = chars['observations']
assert len(cases) == 33 and len({case['function_ordinal'] for case in cases}) == 33
symbols = char_probe['fixtures'][0]['mir']['backend']['debug']['symbols']
trapped_char = next(symbol['function'] for symbol in symbols
                    if symbol['name'].endswith('::value::discardedCharTrap'))
for case in cases:
    assert case['arguments'] == []
    if case['function_ordinal'] == trapped_char:
        assert case['native_status'] == 'trapped' and case['native_result'] is None
    else:
        assert case['native_status'] == 'returned' and case['native_result'] == case['vm_result'] == 42
assert sum(case['native_status'] == 'trapped' for case in cases) == 1
if 'llvm_comparison' in chars:
    comparison = chars['llvm_comparison']
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
probe = json.loads((root / 'equality-probe.json').read_text())
backend = probe['fixtures'][0]['mir']['backend']
ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
               if symbol['name'].endswith('::value::tuples'))
function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
reduction = next(statement['Assign']['value']['Binary']
                 for block in function['blocks'] for statement in block['statements']
                 if statement.get('Assign', {}).get('value', {}).get('Binary', {}).get('operator') == 'logical-and')
reduction['operator'] = 'logical-or'
(root / 'equality-reduction.json').write_text(json.dumps(probe) + '\n')
probe = json.loads((root / 'units-probe.json').read_text())
for name, function_name in [('omitted-unit-call', 'checkedUnit'), ('omitted-empty-call', 'discardedEmpty')]:
    candidate = copy.deepcopy(probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if name == 'omitted-unit-call':
        block = next(block for block in function['blocks']
                     if 'Call' in block['terminator'].get('Invoke', {}).get('operation', {}))
        call = block['terminator']['Invoke']
        if call['destination'] is not None:
            block['statements'].append({'Assign': {'destination': call['destination'],
                                                 'value': {'Use': {'Constant': 'Unit'}}}})
    else:
        block = next(block for block in function['blocks'] if 'CallAggregate' in block['terminator'])
        call = block['terminator']['CallAggregate']
    block['terminator'] = {'Goto': {'target': call['target']}}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
probe = json.loads((root / 'integers-probe.json').read_text())
for name, function_name in [('integer-range', 'overflowSigned8'),
                            ('integer-shift', 'shiftSigned'), ('integer-complement', 'bytes')]:
    candidate = copy.deepcopy(probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if name == 'integer-range':
        guard = next(block['terminator']['Invoke']['operation']['Assert']
                     for block in function['blocks']
                     if isinstance(block['terminator'], dict)
                     and 'Assert' in block['terminator'].get('Invoke', {}).get('operation', {}))
        guard['condition'] = {'Constant': {'Bool': True}}
    else:
        operator = 'subtract' if name == 'integer-shift' else 'bitwise-xor'
        operation = next(statement['Assign']['value']['Binary']
                         for block in function['blocks'] for statement in block['statements']
                         if statement.get('Assign', {}).get('value', {}).get('Binary', {}).get('operator') == operator)
        if name == 'integer-shift':
            operation['operator'] = 'add'
        else:
            operation['right'] = {'Constant': {'Integer': '-1'}}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
for name, function_name in [('sum-tag', 'maybe'), ('sum-inactive', 'clearedOption'),
                            ('sum-propagation', 'forward'), ('sum-conversion', 'narrow')]:
    candidate = copy.deepcopy(sum_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if name == 'sum-tag':
        assignment = next(statement['Assign'] for block in function['blocks']
                          for statement in block['statements']
                          if statement.get('Assign', {}).get('destination') == function['return_fields'][0])
        assignment['value'] = {'Use': {'Constant': {'Integer': '0'}}}
    elif name == 'sum-inactive':
        assignment = next(statement['Assign'] for block in function['blocks']
                          for statement in block['statements']
                          if statement.get('Assign', {}).get('value') == {'Use': {'Constant': {'Bool': False}}})
        assignment['value'] = {'Use': {'Constant': {'Bool': True}}}
    else:
        branch = next(block['terminator']['SwitchBool'] for block in function['blocks']
                      if 'SwitchBool' in block['terminator'])
        branch['condition'] = {'Constant': {'Bool': name == 'sum-propagation'}}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
for name, function_name in [('enum-tag', 'choose'), ('enum-inactive', 'clearInactivePayloads'),
                            ('enum-field', 'recordVariant'), ('enum-propagation', 'forwardFailure')]:
    candidate = copy.deepcopy(enum_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    assignments = [statement['Assign'] for block in function['blocks']
                   for statement in block['statements'] if 'Assign' in statement]
    if name == 'enum-tag':
        assignment = next(row for row in assignments if row['destination'] == function['return_fields'][0])
        assignment['value'] = {'Use': {'Constant': {'Integer': '1'}}}
    elif name == 'enum-inactive':
        assignment = next(row for row in reversed(assignments)
                          if row['value'] == {'Use': {'Constant': {'Bool': False}}})
        assignment['value'] = {'Use': {'Constant': {'Bool': True}}}
    elif name == 'enum-field':
        assignment = next(row for row in assignments
                          if row['value'] == {'Use': {'Constant': {'Integer': '9i32'}}})
        assignment['value'] = {'Use': {'Constant': {'Integer': '10i32'}}}
    else:
        branch = next(block['terminator']['SwitchBool'] for block in function['blocks']
                      if 'SwitchBool' in block['terminator'])
        branch['condition'] = {'Constant': {'Bool': True}}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
for name, function_name in [('union-tag', 'injection'), ('union-widening', 'widen'),
                            ('union-inactive', 'clearInactive'), ('union-generic-tag', 'boxed[Int]'),
                            ('union-propagation', 'forwardFailure')]:
    candidate = copy.deepcopy(union_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    assignments = [statement['Assign'] for block in function['blocks']
                   for statement in block['statements'] if 'Assign' in statement]
    if name == 'union-tag':
        assignment = next(row for row in assignments
                          if row['value'].get('Use', {}).get('Constant', {}).get('Integer') not in [None, '42'])
        assignment['value'] = {'Use': {'Constant': {'Integer': '-1'}}}
    elif name == 'union-widening':
        assignment = next(row for row in assignments if row['destination'] == function['return_fields'][-1])
        assignment['value'] = {'Use': {'Constant': {'Integer': '0'}}}
    elif name == 'union-inactive':
        assignment = next(row for row in assignments
                          if row['value'] == {'Use': {'Constant': {'Integer': '0'}}})
        assignment['value'] = {'Use': {'Constant': {'Integer': '1'}}}
    elif name == 'union-generic-tag':
        comparison = next(row['value']['Binary'] for row in assignments if 'Binary' in row['value'])
        comparison['right'] = {'Constant': {'Integer': '-1'}}
    else:
        branch = next(block['terminator']['SwitchBool'] for block in function['blocks']
                      if 'SwitchBool' in block['terminator'])
        branch['condition'] = {'Constant': {'Bool': True}}
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')
for name, function_name, operator in [
    ('uint64-add', 'add', 'unsigned-add'),
    ('uint64-order', 'unsignedOrder', 'unsigned-less'),
    ('uint64-division', 'divide', 'unsigned-divide'),
    ('uint64-shift', 'rightShift', 'unsigned-shift-right'),
    ('uint64-conversion', 'toSigned', None),
]:
    candidate = copy.deepcopy(uint64_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if operator is None:
        branch = next(block['terminator']['SwitchBool'] for block in function['blocks']
                      if 'SwitchBool' in block['terminator'])
        branch['condition'] = {'Constant': {'Bool': False}}
    else:
        operations = [statement['Assign']['value'].get('Binary', {})
                      for block in function['blocks'] for statement in block['statements']
                      if 'Assign' in statement]
        operations += [block['terminator'].get('Invoke', {}).get('operation', {}).get('CheckedBinary', {})
                       for block in function['blocks'] if isinstance(block['terminator'], dict)]
        operation = next(operation for operation in operations if operation.get('operator') == operator)
        operation['operator'] = operator.removeprefix('unsigned-')
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')

for name, function_name, operator in [
    ('float-width', 'multiply32', 'float32-multiply'),
    ('float-zero', 'negate64', 'float64-negate'),
    ('float-nan', 'nanRelations64', 'float64-not-equal'),
    ('float-unsigned', 'fromUnsigned64', None),
    ('float-range', 'narrow', None),
    ('float-error-order', 'integer64', None),
]:
    candidate = copy.deepcopy(float_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if name in ['float-range', 'float-error-order']:
        branch = next(block['terminator']['SwitchBool'] for block in function['blocks']
                      if 'SwitchBool' in block['terminator'])
        branch['condition'] = {'Constant': {'Bool': True}}
    else:
        operations = [statement['Assign']['value']
                      for block in function['blocks'] for statement in block['statements']
                      if 'Assign' in statement]
        if name == 'float-unsigned':
            conversion = next(op['NumericConversion'] for op in operations if 'NumericConversion' in op)
            assert conversion['source'] == 'UInt64'
            conversion['source'] = 'Int'
        else:
            values = [value for op in operations for value in op.values() if isinstance(value, dict)]
            values += [block['terminator'].get('Invoke', {}).get('operation', {}).get('CheckedBinary', {})
                       for block in function['blocks'] if isinstance(block['terminator'], dict)]
            operation = next(op for op in values if op.get('operator') == operator)
            operation['operator'] = {'float-width': 'float64-multiply', 'float-zero': 'negate',
                                     'float-nan': 'float64-equal'}[name]
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')

def char_constants(node):
    if isinstance(node, dict):
        if 'Char' in node:
            yield node
        for value in node.values():
            yield from char_constants(value)
    elif isinstance(node, list):
        for value in node:
            yield from char_constants(value)

for name, function_name in [
    ('char-width', 'emoji'), ('char-escape', 'zero'), ('char-order', 'order'),
    ('char-inactive', 'variantReplacement'), ('char-omitted-call', 'discardedCharTrap'),
    ('char-invalid-literal', 'emoji'),
]:
    candidate = copy.deepcopy(char_probe)
    backend = candidate['fixtures'][0]['mir']['backend']
    ordinal = next(symbol['function'] for symbol in backend['debug']['symbols']
                   if symbol['name'].endswith('::value::' + function_name))
    function = next(function for function in backend['functions'] if function['ordinal'] == ordinal)
    if name == 'char-order':
        operation = next(statement['Assign']['value']['Binary']
                         for block in function['blocks'] for statement in block['statements']
                         if statement.get('Assign', {}).get('value', {}).get('Binary', {}).get('operator') == 'less')
        operation['operator'] = 'greater'
    elif name == 'char-omitted-call':
        block = next(block for block in function['blocks']
                     if 'Call' in block['terminator'].get('Invoke', {}).get('operation', {}))
        call = block['terminator']['Invoke']
        if call['destination'] is not None:
            block['statements'].append({'Assign': {'destination': call['destination'],
                                                 'value': {'Use': {'Constant': {'Char': "'λ'"}}}}})
        block['terminator'] = {'Goto': {'target': call['target']}}
    else:
        values = char_constants(function)
        value = next(value for value in values if name != 'char-inactive' or value['Char'] == "'\\0'")
        value['Char'] = {'char-width': "'\\u{F642}'", 'char-escape': "'0'",
                         'char-inactive': "'a'", 'char-invalid-literal': "'\\u{D800}'"}[name]
    (root / f'{name}.json').write_text(json.dumps(candidate) + '\n')

PY

for candidate in source-drift unsupported missing-observation oracle-drift empty \
    result-width aliased-results scalar-result-protocol missing-successor \
    missing-template template-admitted duplicate-instance incomplete-instance call-template equality-reduction \
    omitted-unit-call omitted-empty-call integer-range integer-shift integer-complement \
    sum-tag sum-inactive sum-propagation sum-conversion \
    enum-tag enum-inactive enum-field enum-propagation \
    union-tag union-widening union-inactive union-generic-tag union-propagation \
    uint64-add uint64-order uint64-division uint64-shift uint64-conversion \
    float-width float-zero float-nan float-unsigned float-range float-error-order \
    char-width char-escape char-order char-inactive char-omitted-call char-invalid-literal; do
    if "$adapter" "${args[@]}" --probe "$tmp/$candidate.json" --output "$tmp/rejected.json" \
        > "$tmp/$candidate.log" 2>&1; then
        echo "native source scalars: $candidate unexpectedly passed" >&2
        exit 1
    fi
    [[ ! -e "$tmp/rejected.json" ]] || { echo "native source scalars: partial report escaped" >&2; exit 1; }
    if [[ "$candidate" == char-invalid-literal ]]; then
        grep -q 'invalid native Char literal' "$tmp/$candidate.log"
    elif [[ "$candidate" == omitted-*-call || "$candidate" == integer-* || "$candidate" == sum-* || "$candidate" == enum-* || "$candidate" == union-* || "$candidate" == uint64-* || "$candidate" == float-* || "$candidate" == char-* ]]; then
        grep -q 'normalized MIR and hosted VM observations disagree' "$tmp/$candidate.log"
    fi
done
cp "$tmp/report.json" "$target_dir/reliability/evidence/native-source-scalars.json"
cp "$tmp/records-report.json" "$target_dir/reliability/evidence/native-source-records.json"
cp "$tmp/calls-report.json" "$target_dir/reliability/evidence/native-source-calls.json"
cp "$tmp/generics-report.json" "$target_dir/reliability/evidence/native-source-generics.json"
cp "$tmp/equality-report.json" "$target_dir/reliability/evidence/native-source-equality.json"
cp "$tmp/units-report.json" "$target_dir/reliability/evidence/native-source-units.json"
cp "$tmp/integers-report.json" "$target_dir/reliability/evidence/native-source-integers.json"
cp "$tmp/sums-report.json" "$target_dir/reliability/evidence/native-source-sums.json"
cp "$tmp/enums-report.json" "$target_dir/reliability/evidence/native-source-enums.json"
cp "$tmp/unions-report.json" "$target_dir/reliability/evidence/native-source-unions.json"
cp "$tmp/uint64-report.json" "$target_dir/reliability/evidence/native-source-uint64.json"
cp "$tmp/floats-report.json" "$target_dir/reliability/evidence/native-source-floats.json"
cp "$tmp/chars-report.json" "$target_dir/reliability/evidence/native-source-chars.json"
echo "native source scalars: OK (560 Cranelift cases, 68 arithmetic traps, 50 rejected evidence changes)"
if [[ ${#comparison[@]} -gt 0 ]]; then
    echo "native aggregates and generic calls: LLVM comparison OK (515 cases, 65 arithmetic traps)"
fi
