"""Validate target-qualified direct Rust regex measurements and their identity."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
PROBE = 'crates/tondo-stdlib/src/regex/performance.rs'
ROUTES = {
    'compile-literal': 'compile', 'compile-captures': 'compile', 'compile-unicode16': 'compile',
    'find-priority': 'find', 'full-captures': 'full-match', 'captures-unicode': 'captures',
    'find-ambiguous-medium': 'find', 'find-ambiguous-large': 'find',
    'enumerate-dense': 'lazy-enumeration', 'enumerate-empty-unicode': 'lazy-enumeration',
    'enumerate-rescan': 'lazy-enumeration', 'replace-named-first': 'replace-first',
    'replace-named-all': 'replace-all', 'reject-syntax': 'compile', 'reject-program': 'compile',
    'reject-input': 'find', 'reject-steps': 'find', 'reject-matches': 'lazy-enumeration',
    'reject-output': 'replace-all',
}
ERRORS = {
    'reject-syntax': 'UnsupportedFeature', 'reject-program': 'ProgramLimitExceeded',
    'reject-input': 'InputLimitExceeded', 'reject-steps': 'StepLimitExceeded',
    'reject-matches': 'MatchLimitExceeded', 'reject-output': 'OutputLimitExceeded',
}
COUNTERS = {
    'pattern_bytes', 'input_bytes', 'output_bytes', 'operations', 'bytes_copied', 'allocations',
    'logical_memory_bytes', 'program_states', 'semantic_states', 'class_ranges', 'capture_slots',
    'matching_steps', 'matches', 'adversarial_rejections', 'terminal_open_cursors', 'native_live_handles',
}
IDENTITY = ['suite', 'workload_id', 'probe_sha256', 'source_tree_sha256', 'target', 'backend',
            'profile', 'toolchain', 'flags', 'git_revision']
PROTOCOL = {
    'clock': 'monotonic', 'warmup_iterations': 3, 'measurement_repetitions': 9,
    'independent_processes': 3, 'minimum_sample_count': 27, 'batch_operations': 16,
    'fixtures': 'deterministic-bounded-model-and-authored-regex-laws', 'outliers': 'report-not-delete',
    'sample_coordinates': 'process-1-through-3-and-repetition-0-through-8',
    'fixture_setup': 'excluded-from-timed-latency; retained-fixture-identities-included-in-resource-model',
}
STRATEGY = {
    'stdlib_kernel': 'direct-rust-ordered-thompson-nfa-baseline', 'compiler_api': 'not-claimed',
    'hosted_vm': 'not-claimed-no-regex-bridge', 'native_runtime_abi': 'not-measured',
    'native_aot': 'not-claimed', 'simd': 'not-measured-no-optimized-route',
    'multiversion_dispatch': 'not-claimed', 'code_size': 'not-measured', 'selection': 'scalar-only',
}
RESOURCE_MODEL = {
    'copies': 'pattern-copy-or-emitted-replacement-payload; excludes-private-temporary-copies',
    'allocations': 'selected-fixture-program-and-result-identities; not-allocator-calls',
    'logical_memory': 'retained-fixture-plus-program-admission-reservation-and-one-result; not-RSS-or-allocator-peak',
    'matching_steps': 'actual-search-or-cursor-budget-counter-outside-timing; null-for-compile-and-replacement',
    'matches': 'delivered-match-records-per-operation; valid-prefix-preserved-before-cursor-error',
    'automaton': 'actual-instruction-count-semantic-slots-class-ranges-and-capture-slots',
    'excluded': ['oracle-temporaries', 'dependency-parser-and-compile-worklists', 'replacement-token-and-unpublished-output-staging', 'transient-search-allocation-count',
                 'capacity-slack', 'arc-control-blocks', 'other-workloads', 'OS-allocator-and-native-runtime'],
    'terminal_open_cursors': 'lexical-drop-and-independently-checked-fused-cursors',
    'native_live_handles': 'unmeasured-null; no-native-bridge',
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def validate_counters(value, name):
    require(set(value) == COUNTERS, 'counter fields')
    require(value['native_live_handles'] is None, 'native handles are unmeasured')
    require(all(type(item) is int and 0 <= item <= 16_777_216 for field, item in value.items()
                if field not in {'native_live_handles', 'matching_steps'}), 'logical counter type or bound')
    require(0 < value['pattern_bytes'] <= 128 and 0 < value['input_bytes'] <= 4096
            and value['operations'] == 16, 'fixture or batch bound')
    require(value['allocations'] > 0 and value['logical_memory_bytes'] > 0, 'missing resource model')
    require(value['terminal_open_cursors'] == 0, 'live cursor')
    require(value['adversarial_rejections'] == (16 if name in ERRORS else 0), 'rejection count')
    if name in ERRORS:
        require(value['output_bytes'] == 0 and value['bytes_copied'] == 0, 'partial error output')
    emitting = ROUTES[name] in {'replace-first', 'replace-all'} and name not in ERRORS
    require((value['output_bytes'] > 0) == emitting, 'output route')
    transported = value['pattern_bytes'] if ROUTES[name] == 'compile' else value['output_bytes']
    require(value['bytes_copied'] == (0 if name in ERRORS else transported * 16), 'selected payload copies')
    if name in {'reject-syntax', 'reject-program'}:
        require(all(value[field] == 0 for field in ['program_states', 'semantic_states', 'class_ranges', 'capture_slots']),
                'partial published program')
    else:
        require(0 < value['program_states'] <= value['semantic_states'] <= 4096
                and 0 < value['capture_slots'] <= 18 and value['capture_slots'] % 2 == 0, 'automaton shape')
    unmeasured_steps = ROUTES[name] in {'compile', 'replace-first', 'replace-all'}
    if unmeasured_steps:
        require(value['matching_steps'] is None, 'unmeasured matching steps')
    else:
        require(type(value['matching_steps']) is int and 0 <= value['matching_steps'] <= 1_000_000, 'search budget')
    require(value['matches'] == 1 if name == 'reject-matches' else value['matches'] <= 128, 'valid prefix count')


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and not raw.endswith(b'\n\n') and b'\r' not in raw
            and all(line.rstrip() == line for line in raw.splitlines()), 'canonical whitespace')
    value = json.loads(raw)
    require(value['format'] == 'tondo-stdlib-regex-performance/1' and value['task'] == 'STD-REGEX-PERF-001'
            and value['owner'] == 'std.regex' and value['edition'] == '0.1' and value['phase'] == 'STD-0.1B', 'owner')
    require(value['status'] in {'measurement-ready', 'verified-stdlib-kernel-baseline'}, 'status')
    require(value['target'] == 'x86_64-unknown-linux-gnu' and value['backend'] == 'rust-stdlib-kernel'
            and value['profile'] == 'test', 'target or route')
    require(value['contract'] == 'docs/contracts/stdlib-regex-performance.md'
            and value['parent_contract'] == 'testing/stdlib-regex.json', 'contract links')
    probe = value['probe']
    require(set(probe) == {'path', 'test', 'sha256'} and probe['path'] == PROBE
            and probe['test'] == 'regex::performance::regex_performance_probe', 'probe route')
    require(re.fullmatch(r'[0-9a-f]{64}', probe['sha256'])
            and hashlib.sha256((ROOT / PROBE).read_bytes()).hexdigest() == probe['sha256'], 'stale probe')
    require(value['protocol'] == PROTOCOL and value['strategy'] == STRATEGY
            and value['resource_model'] == RESOURCE_MODEL, 'campaign boundary')
    require(value['identity_fields'] == IDENTITY and value['forbidden_identity'] == [
            'ambient_environment', 'cpu_frequency', 'path', 'pid', 'timestamp'], 'identity fields')
    require(len(value['metrics']) == len(set(value['metrics'])) and set(value['metrics']) ==
            COUNTERS - {'pattern_bytes', 'input_bytes', 'output_bytes', 'operations'}
            | {'latency', 'tail_latency', 'throughput', 'dispatch'}, 'metric fields')
    require(value['limits'] == {'fixture_pattern_bytes': 128, 'fixture_input_bytes': 4096,
            'fixture_replacement_bytes': 128, 'reference_input_bytes': 96,
            'reference_input_scalars': 32, 'reference_path_steps': 65536}, 'fixture/reference bounds')
    require(value['oracle'] == {'kind': 'independent-bounded-regex-model-and-authored-unicode-scaling-errors',
            'sources': ['crates/tondo-reliability/src/regex_model.rs',
                        'crates/tondo-reliability/tests/regex_models.rs', PROBE]}, 'oracle')
    require(all((ROOT / source).is_file() for source in value['oracle']['sources']), 'missing oracle source')
    require(value['report'] == 'target/reliability/evidence/stdlib-regex-performance.json', 'report path')
    require(len(value['workloads']) == 19 and {item['id'] for item in value['workloads']} == set(ROUTES), 'workload set')
    for spec in value['workloads']:
        name = spec['id']
        require(set(spec) == {'id', 'operation', 'size_class', 'expected_error', 'dispatch', 'counters'}, 'workload fields')
        require(spec['operation'] == ROUTES[name] and spec['expected_error'] == ERRORS.get(name)
                and spec['dispatch'] == 'scalar-fixed-target', 'workload route')
        require(spec['size_class'] == ('large' if name.endswith('large') else 'medium'
                if name.endswith('medium') or name == 'enumerate-rescan' else 'small'), 'fixture class')
        validate_counters(spec['counters'], name)
    return value


def measurements(contract, rows):
    grouped = defaultdict(list)
    for row in rows:
        require(set(row) == {'workload_id', 'operation', 'process', 'repetition', 'nanos', 'dispatch', 'counters'}, 'sample fields')
        require(row['workload_id'] in ROUTES, 'unknown workload')
        require(type(row['nanos']) is int and row['nanos'] > 0, 'latency')
        require(type(row['process']) is int and 1 <= row['process'] <= 3
                and type(row['repetition']) is int and 0 <= row['repetition'] < 9, 'sample coordinates')
        grouped[row['workload_id']].append(row)
    require(set(grouped) == set(ROUTES), 'missing workload')
    result = []
    for spec in sorted(contract['workloads'], key=lambda spec: spec['id']):
        samples = grouped[spec['id']]
        require(len(samples) == 27 and {(row['process'], row['repetition']) for row in samples} ==
                {(process, repetition) for process in range(1, 4) for repetition in range(9)}, 'missing/duplicated samples')
        for row in samples:
            validate_counters(row['counters'], spec['id'])
            require(row['operation'] == spec['operation'] and row['dispatch'] == spec['dispatch']
                    and row['counters'] == spec['counters'], 'route/counter drift')
        elapsed = sorted(row['nanos'] for row in samples)
        median = elapsed[math.ceil(len(elapsed) * .5) - 1]
        transported = spec['counters']['output_bytes'] or spec['counters']['input_bytes']
        result.append({
            'workload_id': spec['id'], 'operation': spec['operation'], 'size_class': spec['size_class'],
            'expected_error': spec['expected_error'], 'dispatch': spec['dispatch'], 'sample_count': 27,
            'samples': sorted(({'process': row['process'], 'repetition': row['repetition'], 'nanos': row['nanos']}
                               for row in samples), key=lambda row: (row['process'], row['repetition'])),
            'median_ns': median, 'p95_ns': elapsed[math.ceil(len(elapsed) * .95) - 1], 'p99_ns': elapsed[-1],
            'throughput_bytes_per_second': transported * 16_000_000_000 // median,
            'throughput_operations_per_second': 16_000_000_000 // median,
            'counters': spec['counters'],
        })
    return result


def identity(report, name):
    return {field: name if field == 'workload_id' else report[field] for field in IDENTITY}


def validate_report(contract, report, tree):
    require(set(report) == {'format', 'edition', 'phase', 'task', 'suite', 'owner', 'target', 'backend',
            'profile', 'probe_sha256', 'source_tree_sha256', 'git_revision', 'toolchain', 'flags', 'host',
            'capture', 'protocol', 'strategy', 'resource_model', 'measurements'}, 'report fields')
    require(report['format'] == 'tondo-stdlib-regex-performance-report/1'
            and report['suite'] == 'tondo-stdlib-regex-performance', 'report format')
    require(all(report[key] == contract[key] for key in ['edition', 'phase', 'task', 'owner', 'target',
            'backend', 'profile', 'protocol', 'strategy', 'resource_model']), 'report boundary')
    require(report['probe_sha256'] == contract['probe']['sha256'] and re.fullmatch(r'[0-9a-f]{64}', tree)
            and report['source_tree_sha256'] == tree and re.fullmatch(r'[0-9a-f]{40}', report['git_revision']), 'source identity')
    require(set(report['toolchain']) == {'rustc', 'cargo'} and all(type(value) is str for value in report['toolchain'].values())
            and report['toolchain']['rustc'].startswith('rustc 1.93.0 ')
            and report['toolchain']['cargo'].startswith('cargo 1.93.0 '), 'toolchain')
    require(set(report['flags']) == {'rustflags', 'encoded_rustflags', 'incremental'}
            and all(type(value) is str for value in report['flags'].values()), 'build flags')
    require(set(report['host']) == {'cpu_model', 'os'} and all(type(value) is str for value in report['host'].values()), 'host description')
    require(report['capture'] in {'development', 'clean-source'}, 'capture')
    if report['capture'] == 'clean-source':
        committed = subprocess.run(['git', '-C', str(ROOT), 'show', f"{report['git_revision']}:{PROBE}"],
                                   capture_output=True, check=False)
        require(committed.returncode == 0 and hashlib.sha256(committed.stdout).hexdigest() == report['probe_sha256'],
                'capture revision lacks exact probe')
    items = report['measurements']
    require(len(items) == 19 and {item['workload_id'] for item in items} == set(ROUTES), 'report workloads')
    rows = []
    for item in items:
        require(item['identity_sha256'] == digest(identity(report, item['workload_id'])), 'measurement identity')
        for sample in item['samples']:
            require(set(sample) == {'process', 'repetition', 'nanos'}, 'retained sample fields')
            rows.append({'workload_id': item['workload_id'], 'operation': item['operation'],
                         'dispatch': item['dispatch'], 'counters': item['counters'], **sample})
    require([{key: value for key, value in item.items() if key != 'identity_sha256'} for item in items] ==
            measurements(contract, rows), 'measurements, metrics or percentiles drift')
    return report


def render(contract, rows, context):
    report = {'format': 'tondo-stdlib-regex-performance-report/1', 'suite': 'tondo-stdlib-regex-performance',
              'probe_sha256': contract['probe']['sha256'], **{key: contract[key] for key in [
                  'edition', 'phase', 'task', 'owner', 'target', 'backend', 'profile', 'protocol', 'strategy', 'resource_model']},
              **context, 'measurements': measurements(contract, rows)}
    for item in report['measurements']:
        item['identity_sha256'] = digest(identity(report, item['workload_id']))
    return validate_report(contract, report, context['source_tree_sha256'])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=['check-contract', 'render', 'check-report'])
    parser.add_argument('--contract', type=Path, required=True)
    parser.add_argument('--samples', type=Path)
    parser.add_argument('--report', type=Path)
    for key in ['revision', 'tree-sha256', 'target', 'rustc', 'cargo', 'capture']:
        parser.add_argument('--' + key)
    for key in ['rustflags', 'encoded-rustflags', 'incremental', 'cpu-model', 'os']:
        parser.add_argument('--' + key, default='')
    args = parser.parse_args()
    contract = load_contract(args.contract)
    if args.mode == 'check-contract':
        return
    require(args.report is not None and args.tree_sha256, 'report/source inputs required')
    if args.mode == 'check-report':
        validate_report(contract, json.loads(args.report.read_text()), args.tree_sha256)
        return
    require(args.samples is not None and args.revision and args.rustc and args.cargo and args.capture
            and args.target == contract['target'], 'capture inputs')
    rows = [json.loads(line) for line in args.samples.read_text().splitlines()]
    report = render(contract, rows, {'git_revision': args.revision, 'source_tree_sha256': args.tree_sha256,
        'toolchain': {'rustc': args.rustc, 'cargo': args.cargo},
        'flags': {'rustflags': args.rustflags, 'encoded_rustflags': args.encoded_rustflags, 'incremental': args.incremental},
        'host': {'cpu_model': args.cpu_model, 'os': args.os}, 'capture': args.capture})
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + '\n')


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f'std.regex performance: {error}', file=sys.stderr)
        sys.exit(1)
