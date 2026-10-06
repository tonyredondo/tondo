"""Compare public hosted UUID execution with bounded native Rust-kernel cases."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parent.parent
CONTRACT = 'testing/stdlib-uuid-conformance.json'
CORPUS = 'crates/tondo-reliability/tests/fixtures/uuid-cases.json'
STATUS = 'verified-public-hosted-vm-and-native-kernel-process'
CASES = {'retained-values': 17, 'retained-refusals': 35, 'core-value-laws': 3,
         'provider-transcript': 16, 'clock-laws': 6}
KERNEL_ONLY = ['v4-length-17', 'v5-name-limit']
BOUNDARY = {
    'vm': 'compiled-public-tondo-uuid-through-production-host',
    'native': 'rust-stdlib-kernel-process-with-test-provider-replay',
    'compiler_api': 'verified-production-hosted', 'production_host': 'verified-production-hosted',
    'native_abi': 'not-implemented', 'native_aot': 'not-claimed',
    'native_provider_capabilities': 'not-claimed-no-native-tondo-provider-adapter',
    'runtime_objects': 'table-counter-only-no-native-uuid-managed-objects',
    'vm_heap_metrics': 'not-measured', 'simd': 'not-measured-no-optimized-route',
}
RULES = {
    'fresh_process_per_adapter': True, 'same_compiled_fixture_bytes': True,
    'capture_budget_seconds': 480,
    'independent_model_before_vm': True, 'full_reference_vectors': 54,
    'common_valid_vectors': 17, 'common_invalid_vectors': 35,
    'kernel_only_controls': KERNEL_ONLY, 'common_observations': 77,
    'vm_static_capability_checks': 15, 'vm_missing_capability_refusals': 9,
    'vm_capability_acceptances': 6, 'sealed_replay_after_each_static_check': True,
    'native_reference_consumed_rows': {'clock': 8, 'entropy': 9},
    'vm_consumption_proof': 'exact-finite-transcript-and-first-row-replay-not-a-counter',
    'envelope_closed_after_execution': True,
    'partial_failed_capture_promotes': False,
}
SOURCES = [
    'crates/tondo-reliability/examples/uuid_conformance_vm.rs',
    'crates/tondo-native-runtime/examples/uuid_conformance.rs',
    'crates/tondo-stdlib/examples/support/uuid_conformance_cases.rs', CORPUS,
    'crates/tondo-stdlib/src/uuid.rs', 'crates/tondo-reliability/src/uuid_model.rs',
    'crates/tondo-reliability/tests/uuid_models.rs',
    'crates/tondo-compiler/src/driver.rs', 'crates/tondo-compiler/src/package.rs',
    'crates/tondo-compiler/src/process_host.rs', 'crates/tondo-compiler/src/process_host/uuid.rs',
    'crates/tondo-compiler/src/test_control.rs', 'crates/tondo-compiler/src/uuid_provider.rs',
    'crates/tondo-vm/src/bytecode.rs', 'crates/tondo-vm/src/bytecode/verify.rs',
    'crates/tondo-vm/src/runtime/execute.rs', 'crates/tondo-vm/src/runtime/heap.rs',
    'crates/tondo-vm/src/runtime/worker_import.rs',
    'crates/tondo-native-runtime/src/lib.rs', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
    'crates/tondo-reliability/Cargo.toml', 'crates/tondo-native-runtime/Cargo.toml',
    'crates/tondo-compiler/Cargo.toml', 'crates/tondo-stdlib/Cargo.toml', 'crates/tondo-vm/Cargo.toml',
    'scripts/stdlib_uuid_conformance.py',
]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def digest(value):
    return sha(json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode())


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and b'\r' not in raw and
            all(line.rstrip() == line for line in raw.splitlines()), 'contract whitespace')
    contract = json.loads(raw)
    require(type(contract) is dict and set(contract) == {'format', 'edition', 'owner', 'phase', 'task', 'status',
                             'parent', 'document', 'sources', 'boundary', 'rules', 'cases',
                             'report', 'next_blocks'}, 'contract fields')
    require(contract['format'] == 'tondo-stdlib-uuid-conformance/1' and
            contract['edition'] == '0.1' and contract['owner'] == 'std.uuid' and
            contract['phase'] == 'STD-0.1B' and contract['task'] == 'STD-UUID-CONF-001', 'contract identity')
    require(contract['status'] in {'adapter-ready', STATUS}, 'contract status')
    require(contract['parent'] == 'testing/stdlib-uuid.json' and
            contract['document'] == 'docs/contracts/stdlib-uuid-conformance.md' and
            contract['report'] == 'target/reliability/evidence/stdlib-uuid-conformance.json' and
            contract['next_blocks'] == ['STD-UUID-DOC-001'], 'contract links')
    require(contract['sources'] == SOURCES and all((ROOT / path).is_file() for path in SOURCES), 'source list')
    require(digest(contract['boundary']) == digest(BOUNDARY) and digest(contract['rules']) == digest(RULES), 'honest route or corpus boundary')
    require(digest(contract['cases']) == digest([{'id': key, 'observations': count} for key, count in CASES.items()]), 'case identities')
    return contract


def value_observation(raw):
    value = uuid.UUID(raw)
    data = value.bytes
    variant = ('Ncs' if data[8] & 128 == 0 else 'Rfc9562' if data[8] & 64 == 0
               else 'Microsoft' if data[8] & 32 == 0 else 'Future')
    return 'ok:' + str(value) + ''.join(':' + str(byte) for byte in data) + \
        f':{data[6] >> 4}:{variant}:{str(value.int == 0).lower()}:{str(value.int == 2**128-1).lower()}'


def v4(entropy):
    data = bytearray.fromhex(entropy)
    if len(data) != 16:
        return 'err:ProviderMisconfigured'
    data[6] = (data[6] & 15) | 64
    data[8] = (data[8] & 63) | 128
    return value_observation(str(uuid.UUID(bytes=bytes(data))))


def v7(milliseconds, entropy):
    if not 0 <= milliseconds <= 2**48-1:
        return 'err:TimestampOutOfRange'
    data = bytearray.fromhex(entropy)
    if len(data) != 10:
        return 'err:ProviderMisconfigured'
    data = bytearray(milliseconds.to_bytes(6, 'big')) + data
    data[6] = (data[6] & 15) | 112
    data[8] = (data[8] & 63) | 128
    return value_observation(str(uuid.UUID(bytes=bytes(data))))


def expected_observations():
    corpus = json.loads((ROOT / CORPUS).read_bytes())
    require(len(corpus['valid']) == 17 and len(corpus['invalid']) == 37, 'retained corpus sizes')
    valid = [row['id'] + '|' + value_observation(row['value']) for row in corpus['valid']]
    invalid = [row['id'] + '|err:' + row['error'] +
               ('' if row['offset'] is None else ' at byte ' + str(row['offset']))
               for row in corpus['invalid'] if row['id'] not in KERNEL_ONLY]
    nil, maximum = value_observation('00000000-0000-0000-0000-000000000000'), value_observation('ffffffff-ffff-ffff-ffff-ffffffffffff')
    r4, r7 = '919108f752d133205bacf847db4148a8', '0cc318c4dc0c0c07398f'
    transcript = [
        'err:EntropyFailure', 'err:TimestampOutOfRange', 'err:ClockFailure', v4(r4),
        v7(1645557742000, r7), v7(1645557742000, r7), 'err:ProviderMisconfigured',
        v7(0, '00'*10), 'err:EntropyUnavailable', 'err:ClockUnavailable', v4(r4),
        v7(1645557742001, r7), 'err:EntropyUnavailable', 'err:EntropyUnavailable',
        'err:ClockUnavailable', 'err:EntropyUnavailable',
    ]
    return {'retained-values': valid, 'retained-refusals': invalid,
            'core-value-laws': [nil, maximum, 'laws:0:1:-1:1:0:true'],
            'provider-transcript': [f'{index}|{value}' for index, value in enumerate(transcript)],
            'clock-laws': [v7(ms, r7) for ms in [100, 100, 99, 2**48-1, 0]] + ['ordering:0:1:-1:1']}


def expected_capabilities():
    observations = []
    for version, required, replay in [
        (4, ['entropy'], '919108f7-52d1-4320-9bac-f847db4148a8'),
        (7, ['civil-clock', 'entropy'], '017f22e2-79b0-7cc3-98c4-dc0c0c07398f'),
    ]:
        for form in ['direct', 'alias', 'defer']:
            for omitted in required + [None]:
                observations.append({'version': version, 'form': form, 'missing': omitted,
                                     'result': 'accepted' if omitted is None else 'E1008',
                                     'sealed_replay': replay, 'envelope_closed': True})
    return observations


def compare(vm_text, native_text):
    vm, native = [json.loads(line) for line in vm_text.splitlines()], [json.loads(line) for line in native_text.splitlines()]
    expected = expected_observations()
    vm_expected = [{'id': key, 'observations': lines, 'envelopes_closed': True} for key, lines in expected.items()]
    vm_expected.append({'id': 'vm-capabilities', 'observations': expected_capabilities()})
    native_expected = [{'id': key, 'observations': lines, 'live_runtime_objects': 0} for key, lines in expected.items()]
    require(digest(vm) == digest(vm_expected), 'public VM observations or static capability evidence differ')
    require(digest(native) == digest(native_expected), 'native kernel observations or table counters differ')
    return {'vm_stdout': vm_text, 'native_stdout': native_text, 'vm_cases': vm,
            'native_cases': native, 'vm_log_sha256': sha(vm_text.encode()), 'native_log_sha256': sha(native_text.encode())}


def git(*arguments):
    return subprocess.check_output(['git', *arguments], cwd=ROOT, stderr=subprocess.PIPE)


def snapshot():
    return {'revision': git('rev-parse', 'HEAD').decode().strip(),
            'git_tree': git('rev-parse', 'HEAD^{tree}').decode().strip(),
            'dirty': bool(git('status', '--porcelain')),
            'sources': {path: sha((ROOT / path).read_bytes()) for path in SOURCES}}


def metadata_only(paths):
    require(all(path.endswith('.md') or (path.startswith('testing/') and path.endswith('.json'))
                for path in paths), 'executable or build input differs from capture')


def validate_report(report, contract, contract_raw, development=False):
    require(type(report) is dict and set(report) == {'format', 'task', 'status', 'qualification', 'source', 'contract_sha256',
                            'boundary', 'comparison', 'tests', 'identity_sha256'}, 'report fields')
    require(report['format'] == 'tondo-stdlib-uuid-conformance-evidence/1' and
            report['task'] == 'STD-UUID-CONF-001' and report['status'] == 'passed', 'report identity')
    require(report['contract_sha256'] == sha(contract_raw) and digest(report['boundary']) == digest(BOUNDARY), 'contract binding')
    qualification = report['qualification']
    require(type(qualification) is dict and set(qualification) == {'target', 'toolchain', 'profile', 'flags'} and
            qualification['profile'] == 'dev' and qualification['target'] == 'x86_64-unknown-linux-gnu' and
            type(qualification['toolchain']) is str and qualification['toolchain'].startswith('rustc 1.93.0') and
            qualification['flags'] in [{'CARGO_INCREMENTAL': ''}, {'CARGO_INCREMENTAL': '0'}, {'CARGO_INCREMENTAL': '1'}], 'build qualification')
    source = report['source']
    require(type(source) is dict and set(source) == {'revision', 'git_tree', 'dirty', 'sources'} and
            type(source['dirty']) is bool and type(source['sources']) is dict and set(source['sources']) == set(SOURCES) and
            type(source['revision']) is str and type(source['git_tree']) is str and
            re.fullmatch('[0-9a-f]{40}', source['revision']) and re.fullmatch('[0-9a-f]{40}', source['git_tree']), 'source fields')
    require(not source['dirty'] or development, 'development capture cannot promote')
    for path in SOURCES:
        require(source['sources'][path] == sha((ROOT / path).read_bytes()), 'current source binding: ' + path)
        if not source['dirty']:
            require(source['sources'][path] == sha(git('show', source['revision'] + ':' + path)), 'committed source binding: ' + path)
    require(source['git_tree'] == git('rev-parse', source['revision'] + '^{tree}').decode().strip(), 'Git tree binding')
    if not source['dirty']:
        metadata_only(git('diff', '--name-only', source['revision'], '--').decode().splitlines())
        metadata_only(git('ls-files', '--others', '--exclude-standard').decode().splitlines())
    comparison = report['comparison']
    require(type(comparison) is dict and set(comparison) == {'vm_stdout', 'native_stdout', 'vm_cases', 'native_cases', 'vm_log_sha256', 'native_log_sha256'} and
            type(comparison['vm_stdout']) is str and type(comparison['native_stdout']) is str, 'comparison fields')
    require(digest(compare(comparison['vm_stdout'], comparison['native_stdout'])) == digest(comparison), 'comparison binding')
    require(report['tests'] == {'kernel': 'passed', 'independent_model': 'passed', 'public_vm': 'passed', 'host_regressions': 'passed'}, 'executed prerequisites')
    require(report['identity_sha256'] == digest({key: value for key, value in report.items() if key != 'identity_sha256'}), 'evidence identity')


def capture(contract_path, output):
    contract, raw = load_contract(contract_path), contract_path.read_bytes()
    for key, value in os.environ.items():
        override = key in {'RUSTC', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTFLAGS'} or key.startswith(('CARGO_PROFILE_DEV_', 'CARGO_PROFILE_TEST_')) or (key.startswith('CARGO_TARGET_') and key.endswith(('_RUSTFLAGS', '_LINKER')))
        require(not override or not value, 'unsupported build override: ' + key)
    before = snapshot()
    require(not before['dirty'] or os.environ.get('TONDO_STDLIB_UUID_CONF_ALLOW_DIRTY') == '1', 'clean workspace required; explicit development override only')
    logs = output.parent / 'stdlib-uuid-conformance-logs'
    logs.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + RULES['capture_budget_seconds']
    def run(name, arguments):
        remaining = deadline - time.monotonic()
        require(remaining > 0, 'capture time budget exhausted')
        process = subprocess.run(['cargo', *arguments, '--locked'], cwd=ROOT, capture_output=True, timeout=min(180, remaining))
        (logs / (name + '.stdout')).write_bytes(process.stdout)
        (logs / (name + '.stderr')).write_bytes(process.stderr)
        require(process.returncode == 0, name + ' failed; see retained logs')
        return process.stdout.decode()
    vm = run('vm', ['run', '-q', '-p', 'tondo-reliability', '--example', 'uuid_conformance_vm'])
    native = run('native', ['run', '-q', '-p', 'tondo-native-runtime', '--example', 'uuid_conformance'])
    comparison = compare(vm, native)
    run('kernel', ['test', '-q', '-p', 'tondo-stdlib', '--lib', 'uuid::tests'])
    run('model', ['test', '-q', '-p', 'tondo-reliability', '--test', 'uuid_models'])
    run('public-vm', ['test', '-q', '-p', 'tondo-reliability', '--example', 'uuid_conformance_vm'])
    run('host', ['test', '-q', '-p', 'tondo-compiler', '--lib', 'uuid_'])
    run('host-vm', ['test', '-q', '-p', 'tondo-vm', '--lib', 'uuid'])
    require(snapshot() == before and contract_path.read_bytes() == raw, 'capture inputs changed')
    toolchain = subprocess.check_output(['rustc', '-vV'], cwd=ROOT).decode().strip()
    target = next(line.removeprefix('host: ') for line in toolchain.splitlines() if line.startswith('host: '))
    report = {'format': 'tondo-stdlib-uuid-conformance-evidence/1', 'task': 'STD-UUID-CONF-001', 'status': 'passed',
              'qualification': {'target': target, 'toolchain': toolchain, 'profile': 'dev', 'flags': {'CARGO_INCREMENTAL': os.environ.get('CARGO_INCREMENTAL', '')}},
              'source': before, 'contract_sha256': sha(raw), 'boundary': BOUNDARY, 'comparison': comparison,
              'tests': {'kernel': 'passed', 'independent_model': 'passed', 'public_vm': 'passed', 'host_regressions': 'passed'}}
    report['identity_sha256'] = digest(report)
    validate_report(report, contract, raw, before['dirty'])
    temporary = output.with_suffix(output.suffix + '.tmp')
    temporary.write_text(json.dumps(report, indent=2) + '\n')
    os.replace(temporary, output)
    print('std.uuid conformance: OK (5 shared groups; 77 exact observations; 15 public VM capability checks; ' + ('development' if before['dirty'] else 'clean-source') + ')')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check-contract', 'capture', 'check-report'])
    parser.add_argument('--contract', type=Path, default=ROOT / CONTRACT)
    parser.add_argument('--report', type=Path)
    parser.add_argument('--allow-development', action='store_true')
    arguments = parser.parse_args()
    contract = load_contract(arguments.contract)
    if arguments.action == 'capture':
        require(arguments.report is not None, 'report path required')
        capture(arguments.contract, arguments.report)
    elif arguments.action == 'check-report':
        require(arguments.report is not None, 'report path required')
        validate_report(json.loads(arguments.report.read_bytes()), contract, arguments.contract.read_bytes(), arguments.allow_development)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, TypeError, KeyError, OSError, subprocess.SubprocessError) as error:
        print('std.uuid conformance: ' + str(error), file=sys.stderr)
        sys.exit(1)
