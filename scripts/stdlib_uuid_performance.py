"""Validate bounded, target-qualified UUID hosted bridge measurements."""
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
PROBE = 'crates/tondo-compiler/src/process_host/uuid/performance.rs'
ROUTES = {
    'parse-dashed': 'parse', 'parse-urn': 'parse', 'format-canonical': 'format',
    'bytes-roundtrip': 'bytes-roundtrip', 'v4-sealed': 'v4', 'v4-os': 'v4',
    'v5-empty': 'v5', 'v5-small': 'v5', 'v5-two-block': 'v5', 'v5-large': 'v5',
    'v7-sealed': 'v7', 'v7-os': 'v7', 'reject-text-length': 'parse', 'reject-character': 'parse',
    'reject-name-limit': 'v5', 'reject-entropy-error': 'v4', 'reject-entropy-shape': 'v4',
    'reject-clock-error': 'v7', 'reject-clock-negative': 'v7', 'reject-clock-high': 'v7',
    'reject-provider-limit': 'v4', 'reject-reply-budget': 'v4',
}
ERRORS = {
    'reject-text-length': 'InvalidTextLength', 'reject-character': 'InvalidCharacter',
    'reject-name-limit': 'NameLimitExceeded', 'reject-entropy-error': 'EntropyFailure',
    'reject-entropy-shape': 'ProviderMisconfigured', 'reject-clock-error': 'ClockFailure',
    'reject-clock-negative': 'TimestampOutOfRange', 'reject-clock-high': 'TimestampOutOfRange',
    'reject-provider-limit': 'ResourceLimit', 'reject-reply-budget': 'VmMemoryResourceLimit',
}
COUNTERS = {
    'input_bytes', 'output_bytes', 'operations', 'bytes_copied', 'allocations',
    'logical_memory_bytes', 'reply_admission_bytes', 'reply_retained_bytes',
    'host_handles_created', 'terminal_live_handles', 'terminal_budget_bytes',
    'entropy_requests', 'clock_requests', 'sha1_blocks', 'adversarial_rejections', 'native_live_handles',
}
IDENTITY = ['suite', 'workload_id', 'probe_sha256', 'source_tree_sha256', 'target', 'backend',
            'profile', 'toolchain', 'flags', 'git_revision']
PROTOCOL = {
    'clock': 'monotonic', 'warmup_iterations': 3, 'measurement_repetitions': 9,
    'independent_processes': 3, 'minimum_sample_count': 27, 'batch_operations': 16,
    'fixtures': 'independent-bounded-model-authored-name-digest-and-OS-layout-laws',
    'outliers': 'report-not-delete', 'sample_coordinates': 'process-1-through-3-and-repetition-0-through-8',
    'fixture_setup': 'excluded-from-timed-latency; retained-fixture-identities-included-in-resource-model',
}
STRATEGY = {
    'selected_route': 'hosted-scalar', 'compiler_api': 'inherited-verified-production-hosted',
    'hosted_bridge': 'direct-private-bootstrap-host-with-reply-admission',
    'hosted_vm': 'not-measured-no-bytecode-execution', 'native_runtime_abi': 'not-measured',
    'native_aot': 'not-measured', 'simd': 'not-measured-no-optimized-route',
    'multiversion_dispatch': 'not-claimed', 'code_size': 'not-measured',
    'providers': 'finite-sealed-snapshots-and-explicit-OS-getrandom-SystemTime',
}
RESOURCE_MODEL = {
    'copies': 'selected-format-byte-roundtrip-and-sealed-entropy-payload-transport-per-batch',
    'allocations': 'selected-retained-fixture-and-result-owned-identities; not-allocator-calls',
    'logical_memory': 'selected-retained-fixture-plus-reply-admission-and-roundtrip-overlap; not-RSS-or-allocator-peak',
    'reply_retained_bytes': 'actual-maximum-simultaneous-response-charges-after-return; excludes-host-buffers',
    'reply_admission_bytes': 'tested-fixed-envelope-reservation-per-call; zero-on-admission-refusal',
    'host_handles_created': 'actual-host-registry-id-increment-during-batch; excludes-input-setup',
    'terminal_live_handles': 'actual-empty-host-registry-and-buffer-charge-table-after-private-Rust-cleanup',
    'terminal_budget_bytes': 'actual-live-bytes-in-shared-VM-host-admission-budget-after-cleanup',
    'provider_requests': 'logical-bridge-provider-invocations-per-batch; sealed-row-consumption-verified; not-OS-syscalls',
    'sha1_blocks': 'modeled-name-digest-compression-blocks-per-successful-v5-operation',
    'native_live_handles': 'unmeasured-null; no-native-UUID-bridge',
    'excluded': ['oracle-temporaries', 'unpublished-reply-preview-and-staging', 'stack-frames-and-hash-state',
                 'capacity-slack', 'host-map-and-Arc-control-blocks', 'other-workloads', 'OS-allocator-and-native-runtime'],
}
LIMITS = {'fixture_name_bytes': 4096, 'fixture_text_bytes': 4096, 'reference_name_bytes': 96,
          'reference_provider_rows': 16, 'batch_operations': 16}
ORACLE = {'kind': 'independent-bounded-UUID-model-authored-large-name-digest-and-OS-layout-laws',
          'sources': ['crates/tondo-reliability/src/uuid_model.rs',
                      'crates/tondo-reliability/tests/uuid_models.rs', PROBE],
          'large_v5': {'namespace': '6ba7b810-9dad-11d1-80b4-00c04fd430c8',
                       'name': 'bytes(index % 251 for index in range(4096))',
                       'expected': '788dc126-8af8-5603-9087-9ddc041c9750',
                       'authoring': 'Python-stdlib-hashlib-SHA1-independent-of-Rust-kernel'},
          'OS_outputs': 'nondeterministic; version-and-variant-laws-only-no-exact-byte-or-quality-oracle'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def validate_counters(value, name):
    require(set(value) == COUNTERS, 'counter fields')
    require(value['native_live_handles'] is None, 'native handles are unmeasured')
    require(all(type(item) is int and 0 <= item <= 16_777_216 for field, item in value.items()
                if field != 'native_live_handles'), 'logical counter type or bound')
    require(value['operations'] == 16 and value['input_bytes'] <= 4112, 'fixture or batch bound')
    require(value['allocations'] > 0 and value['logical_memory_bytes'] > 0, 'missing resource model')
    require(value['terminal_live_handles'] == value['terminal_budget_bytes'] == 0, 'terminal lifecycle')
    reject = name in ERRORS
    require(value['adversarial_rejections'] == (16 if reject else 0), 'rejection count')
    require(value['output_bytes'] == (0 if reject else 36 if name == 'format-canonical' else 16), 'partial or wrong output')
    copies = 36 * 16 if name == 'format-canonical' else 32 * 16 if name == 'bytes-roundtrip' else 16 * 16 if name == 'v4-sealed' else 10 * 16 if name == 'v7-sealed' else 0
    require(value['bytes_copied'] == copies, 'selected payload copies')
    require(value['reply_admission_bytes'] == (0 if name == 'reject-reply-budget' else 182), 'reply admission')
    expected_reply = 0 if name == 'reject-reply-budget' else 68 if name == 'format-canonical' else 164 if name == 'bytes-roundtrip' else 182 if name == 'reject-character' else 150 if reject else 132
    require(value['reply_retained_bytes'] == expected_reply, 'reply charge')
    require(value['host_handles_created'] == (16 if name == 'bytes-roundtrip' else 0), 'host handle count')
    clocks = 16 if ROUTES[name] == 'v7' else 0
    entropy = 16 if name in {'v4-os', 'v4-sealed', 'v7-os', 'v7-sealed', 'reject-entropy-error', 'reject-entropy-shape'} else 0
    require(value['clock_requests'] == clocks and value['entropy_requests'] == entropy, 'provider invocation count')
    blocks = (value['input_bytes'] + 9 + 63) // 64 if ROUTES[name] == 'v5' and not reject else 0
    require(value['sha1_blocks'] == blocks, 'name digest blocks')


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and not raw.endswith(b'\n\n') and b'\r' not in raw
            and all(line.rstrip() == line for line in raw.splitlines()), 'canonical whitespace')
    value = json.loads(raw)
    require(set(value) == {'format', 'edition', 'phase', 'task', 'owner', 'status', 'target', 'backend',
            'profile', 'contract', 'parent_contract', 'probe', 'protocol', 'identity_fields',
            'forbidden_identity', 'metrics', 'workloads', 'limits', 'oracle', 'strategy', 'resource_model', 'report'}, 'contract fields')
    require(value['format'] == 'tondo-stdlib-uuid-performance/1' and value['task'] == 'STD-UUID-PERF-001'
            and value['owner'] == 'std.uuid' and value['edition'] == '0.1' and value['phase'] == 'STD-0.1B', 'owner')
    require(value['status'] in {'measurement-ready', 'verified-hosted-scalar-baseline'}, 'status')
    require(value['target'] == 'x86_64-unknown-linux-gnu' and value['backend'] == 'rust-hosted-bridge'
            and value['profile'] == 'test', 'target or route')
    require(value['contract'] == 'docs/contracts/stdlib-uuid-performance.md'
            and value['parent_contract'] == 'testing/stdlib-uuid.json', 'contract links')
    probe = value['probe']
    require(set(probe) == {'path', 'test', 'sha256'} and probe['path'] == PROBE
            and probe['test'] == 'process_host::uuid::performance::uuid_performance_probe', 'probe route')
    require(re.fullmatch(r'[0-9a-f]{64}', probe['sha256'])
            and hashlib.sha256((ROOT / PROBE).read_bytes()).hexdigest() == probe['sha256'], 'stale probe')
    require(value['protocol'] == PROTOCOL and value['strategy'] == STRATEGY
            and value['resource_model'] == RESOURCE_MODEL and value['limits'] == LIMITS and value['oracle'] == ORACLE, 'campaign boundary')
    require(all((ROOT / source).is_file() for source in value['oracle']['sources']), 'missing oracle source')
    require(value['identity_fields'] == IDENTITY and value['forbidden_identity'] == [
            'ambient_environment', 'cpu_frequency', 'path', 'pid', 'timestamp'], 'identity fields')
    require(len(value['metrics']) == len(set(value['metrics'])) and set(value['metrics']) ==
            COUNTERS - {'input_bytes', 'output_bytes', 'operations'} | {'latency', 'tail_latency', 'throughput', 'dispatch'}, 'metric fields')
    require(value['report'] == 'target/reliability/evidence/stdlib-uuid-performance.json', 'report path')
    require(len(value['workloads']) == len(ROUTES) and {item['id'] for item in value['workloads']} == set(ROUTES), 'workload set')
    for spec in value['workloads']:
        name = spec['id']
        require(set(spec) == {'id', 'operation', 'size_class', 'expected_error', 'dispatch', 'counters'}, 'workload fields')
        require(spec['operation'] == ROUTES[name] and spec['expected_error'] == ERRORS.get(name)
                and spec['dispatch'] == 'hosted-scalar', 'workload route')
        require(spec['size_class'] == ('large' if name in {'v5-large', 'reject-text-length'} else 'medium'
                if name in {'v5-two-block', 'reject-name-limit'} else 'small'), 'fixture class')
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
        transported = (spec['counters']['input_bytes'] if spec['operation'] in {'parse', 'v5'}
                       else spec['counters']['output_bytes'] or spec['counters']['input_bytes'])
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
    require(report['format'] == 'tondo-stdlib-uuid-performance-report/1'
            and report['suite'] == 'tondo-stdlib-uuid-performance', 'report format')
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
    require(len(items) == len(ROUTES) and {item['workload_id'] for item in items} == set(ROUTES), 'report workloads')
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
    report = {'format': 'tondo-stdlib-uuid-performance-report/1', 'suite': 'tondo-stdlib-uuid-performance',
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
        print(f'std.uuid performance: {error}', file=sys.stderr)
        sys.exit(1)
