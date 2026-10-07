"""Compare public hosted NET execution with bounded native Rust-kernel cases."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent.parent
CONTRACT = 'testing/stdlib-net-conformance.json'
CORPUS = 'crates/tondo-reliability/tests/fixtures/net-cases.json'
CASES = {'values-and-keys': 28, 'tcp-transcript': 7, 'udp-transcript': 4, 'dns-transcript': 2}
VM_ONLY = {'tls12-transcript': ['write:4', 'read:706f', 'read:6e67', 'eof:close-notify'],
           'tls12-refusal': ['refused:CertificateRejected'],
           'tls13-transcript': ['write:4', 'read:706f', 'read:6e67', 'eof:close-notify'],
           'tls13-refusal': ['refused:CertificateRejected'],
           'vm-lifecycle': ['listener:assigned-port-closed', 'listener:accepted-address-laws-closed', 'select:losing-datagram-retained'],
           'vm-deadlines': ['deadline:validation-first', 'deadline:expired-no-DNS']}
STATUS = 'verified-public-hosted-vm-and-native-kernel-process'
BOUNDARY = {
    'vm': 'compiled-public-tondo-net-through-production-host',
    'native': 'rust-stdlib-kernel-process-with-finite-model-admission',
    'compiler_api': 'inherited-verified-production-hosted',
    'production_host': 'inherited-verified-production-hosted',
    'native_abi': 'not-implemented', 'native_aot': 'not-claimed',
    'native_network_provider': 'not-implemented-no-native-tondo-network-target',
    'tls_reference': 'VM-only-real-local-TLS-and-independent-verdict-owner-model',
    'runtime_objects': 'table-counter-only-no-native-network-managed-objects',
    'vm_heap_metrics': 'not-measured', 'simd': 'not-measured-no-optimized-route',
}
RULES = {
    'fresh_process_per_adapter': True, 'same_compiled_fixture_bytes': True,
    'capture_budget_seconds': 480, 'independent_model_before_vm': True,
    'full_reference_vectors': 40, 'common_observations': 41,
    'vm_only_observations': 15, 'vm_static_capability_checks': 6,
    'vm_missing_capability_refusals': 3, 'vm_capability_acceptances': 3,
    'providers': 'controlled-loopback-no-external-service',
    'resolver_configuration': 'explicit-per-fixture-endpoint-no-system-fallback',
    'ephemeral_address_normalization': 'after-exact-address-and-payload-law-checks',
    'TLS': ['1.2', '1.3'], 'peer_wait_seconds': 5,
    'process_scope': 'explicit-delegated-scope',
    'kernel_only_controls': 'malformed-provider-reports-and-internal-deadline-domains-retained-in-40-case-prerequisite',
    'envelope_closed_after_execution': True, 'partial_failed_capture_promotes': False,
}
SOURCES = [
    'crates/tondo-reliability/examples/net_conformance_vm.rs',
    'crates/tondo-native-runtime/examples/net_conformance.rs',
    'crates/tondo-stdlib/examples/support/net_conformance_cases.rs', CORPUS,
    'crates/tondo-stdlib/src/net.rs', 'crates/tondo-reliability/src/net_model.rs',
    'crates/tondo-reliability/src/net_model/admission.rs',
    'crates/tondo-reliability/src/net_model/stream.rs',
    'crates/tondo-reliability/src/net_model/tls.rs',
    'crates/tondo-reliability/tests/net_kernel_models.rs',
    'crates/tondo-reliability/tests/net_corpus.rs',
    'crates/tondo-reliability/tests/net_hosted_models.rs',
    'crates/tondo-compiler/src/driver.rs', 'crates/tondo-compiler/src/package.rs',
    'crates/tondo-compiler/src/process_host.rs', 'crates/tondo-compiler/src/process_host/net.rs',
    'crates/tondo-compiler/src/test_control.rs', 'crates/tondo-compiler/src/net_provider.rs',
    'crates/tondo-compiler/src/net_provider/executor.rs',
    'crates/tondo-compiler/src/net_provider/dns_runtime.rs',
    'crates/tondo-compiler/src/net_provider/transport.rs',
    'crates/tondo-compiler/src/net_provider/tls_transport.rs',
    'crates/tondo-compiler/src/net_provider/fixtures/localhost-cert.pem',
    'crates/tondo-compiler/src/net_provider/fixtures/localhost-test-key.pem',
    'crates/tondo-vm/src/bytecode.rs', 'crates/tondo-vm/src/bytecode/verify.rs',
    'crates/tondo-vm/src/runtime/execute.rs', 'crates/tondo-vm/src/runtime/heap.rs',
    'crates/tondo-vm/src/runtime/worker_import.rs',
    'crates/tondo-native-runtime/src/lib.rs', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
    'crates/tondo-reliability/Cargo.toml', 'crates/tondo-native-runtime/Cargo.toml',
    'crates/tondo-compiler/Cargo.toml', 'crates/tondo-stdlib/Cargo.toml', 'crates/tondo-vm/Cargo.toml',
    'scripts/stdlib_net_conformance.py',
]


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return 'sha256:' + hashlib.sha256(data).hexdigest()


def digest(value):
    return sha(json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode())


def load_json(raw):
    def unique_fields(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'duplicate JSON field: ' + key)
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=unique_fields)


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b'\n') and b'\r' not in raw and
            all(line.rstrip() == line for line in raw.splitlines()), 'contract whitespace')
    contract = load_json(raw)
    require(type(contract) is dict and set(contract) == {'format', 'edition', 'owner', 'phase', 'task', 'status',
                             'parent', 'document', 'sources', 'boundary', 'rules', 'cases',
                             'report', 'next_blocks'}, 'contract fields')
    require(contract['format'] == 'tondo-stdlib-net-conformance/1' and
            contract['edition'] == '0.1' and contract['owner'] == 'std.net' and
            contract['phase'] == 'STD-0.1B' and contract['task'] == 'STD-NET-CONF-001', 'contract identity')
    require(contract['status'] in {'adapter-ready', STATUS}, 'contract status')
    require(contract['parent'] == 'testing/stdlib-net.json' and
            contract['document'] == 'docs/contracts/stdlib-net-conformance.md' and
            contract['report'] == 'target/reliability/evidence/stdlib-net-conformance.json' and
            contract['next_blocks'] == ['STD-NET-DOC-001'], 'contract links')
    require(contract['sources'] == SOURCES and all((ROOT / path).is_file() for path in SOURCES), 'source list')
    require(digest(contract['boundary']) == digest(BOUNDARY) and digest(contract['rules']) == digest(RULES), 'honest route or corpus boundary')
    require(digest(contract['cases']) == digest([{'id': key, 'observations': count} for key, count in CASES.items()]), 'case identities')
    return contract


def expected_observations():
    corpus = load_json((ROOT / CORPUS).read_bytes())
    require(len(corpus['cases']) == 40, 'retained corpus size')
    values = [row['id'] + ('|err:' + row['error'] if 'error' in row else '|ok')
              for row in corpus['cases'][:13]]
    values += [key + '|ok' for key in ['ip-loopback-v4', 'ip-loopback-v6', 'ip-mapped-v6', 'ip-expanded-v6']]
    values += [key + '|err:InvalidAddress' for key in ['ip-name-not-resolution', 'ip-scope-refusal', 'ip-brackets-refusal', 'ip-leading-space']]
    values += ['port-zero|ok', 'port-maximum|ok', 'port-negative|err:InvalidPort',
               'port-overflow|err:InvalidPort', 'host-key:17', 'ip-key:19', 'socket-key:23']
    return {'values-and-keys': values,
            'tcp-transcript': ['tcp:0:data:61', 'tcp:1:data:62', 'tcp:2:data:63', 'tcp:3:eof',
                               'tcp:write:0:1', 'tcp:write:1:1', 'tcp:write:2:1'],
            'udp-transcript': ['udp:0:ok:616263', 'udp:1:ok:', 'udp:2:err:DatagramTooLarge', 'udp:3:ok:6f6b'],
            'dns-transcript': ['dns:2:err:ResourceLimit', 'dns:3:ordered-v4-v6']}


def expected_capabilities():
    return [{'form': form, 'network': enabled, 'result': 'accepted' if enabled else 'E1008'}
            for form in ['import', 'alias', 'defer'] for enabled in [False, True]]


def compare(vm_text, native_text):
    vm, native = [load_json(line) for line in vm_text.splitlines()], [load_json(line) for line in native_text.splitlines()]
    expected = expected_observations()
    vm_expected = [{'id': key, 'observations': lines, 'envelopes_closed': True} for key, lines in expected.items()]
    vm_expected += [{'id': key, 'observations': lines, 'envelopes_closed': True} for key, lines in VM_ONLY.items()]
    vm_expected.append({'id': 'vm-capabilities', 'observations': expected_capabilities()})
    native_expected = [{'id': key, 'observations': lines, 'live_runtime_objects': 0} for key, lines in expected.items()]
    require(digest(vm) == digest(vm_expected), 'public VM observations or static capability evidence differ')
    require(digest(native) == digest(native_expected), 'native kernel observations or table counters differ')
    return {'vm_stdout': vm_text, 'native_stdout': native_text, 'vm_cases': vm,
            'native_cases': native, 'vm_log_sha256': sha(vm_text.encode()), 'native_log_sha256': sha(native_text.encode())}


def git(*arguments):
    return subprocess.check_output(['git', *arguments], cwd=ROOT, stderr=subprocess.PIPE)


def snapshot(deadline=None):
    return {'revision': git('rev-parse', 'HEAD').decode().strip(),
            'git_tree': git('rev-parse', 'HEAD^{tree}').decode().strip(),
            'dirty': bool(git('status', '--porcelain')),
            'sources': {path: sha((ROOT / path).read_bytes()) for path in SOURCES},
            'quality_inputs': source_provenance(deadline)}


def source_provenance(deadline=None):
    remaining = 180 if deadline is None else min(180, deadline - time.monotonic())
    require(remaining > 0, 'capture time budget exhausted')
    return load_json(subprocess.check_output(
        ['cargo', 'run', '-q', '-p', 'tondo-reliability', '--locked', '--',
         'quality', 'provenance', '--root', '.'], cwd=ROOT, stderr=subprocess.PIPE,
        timeout=remaining))


def metadata_only(paths):
    require(all(path.endswith('.md') or (path.startswith('testing/') and path.endswith('.json'))
                for path in paths), 'executable or build input differs from capture')


def check_build_environment():
    for key, value in os.environ.items():
        override = key in {'RUSTC', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_BUILD_RUSTC', 'CARGO_BUILD_RUSTFLAGS'} or key.startswith(('CARGO_PROFILE_DEV_', 'CARGO_PROFILE_TEST_')) or (key.startswith('CARGO_TARGET_') and key.endswith(('_RUSTFLAGS', '_LINKER')))
        require(not override or not value, 'unsupported build override: ' + key)


def check_process_scope(deadline):
    declared = os.environ.get('TONDO_TEST_PROCESS_CGROUP')
    require(bool(declared), 'explicit delegated process scope required')
    remaining = min(30, deadline - time.monotonic())
    require(remaining > 0, 'capture time budget exhausted')
    actual = subprocess.check_output(
        ['bash', 'scripts/test-process-scope.sh', 'bash', '-c',
         'printf "%s" "$TONDO_TEST_PROCESS_CGROUP"'], cwd=ROOT,
        stderr=subprocess.PIPE, timeout=remaining).decode()
    require(declared == actual, 'declared process scope differs from actual delegated scope')


def validate_report(report, contract, contract_raw, development=False, deadline=None):
    check_build_environment()
    require(type(report) is dict and set(report) == {'format', 'task', 'status', 'qualification', 'source', 'contract_sha256',
                            'boundary', 'comparison', 'tests', 'identity_sha256'}, 'report fields')
    require(report['format'] == 'tondo-stdlib-net-conformance-evidence/1' and
            report['task'] == 'STD-NET-CONF-001' and report['status'] == 'passed', 'report identity')
    require(report['contract_sha256'] == sha(contract_raw) and digest(report['boundary']) == digest(BOUNDARY), 'contract binding')
    qualification = report['qualification']
    require(type(qualification) is dict and set(qualification) == {'target', 'toolchain', 'profile', 'flags'} and
            qualification['profile'] == 'dev' and qualification['target'] == 'x86_64-unknown-linux-gnu' and
            type(qualification['toolchain']) is str and qualification['toolchain'].startswith('rustc 1.93.0') and
            qualification['flags'] in [{'CARGO_INCREMENTAL': ''}, {'CARGO_INCREMENTAL': '0'}, {'CARGO_INCREMENTAL': '1'}], 'build qualification')
    source = report['source']
    require(type(source) is dict and set(source) == {'revision', 'git_tree', 'dirty', 'sources', 'quality_inputs'} and
            type(source['dirty']) is bool and type(source['sources']) is dict and set(source['sources']) == set(SOURCES) and
            type(source['revision']) is str and type(source['git_tree']) is str and
            re.fullmatch('[0-9a-f]{40}', source['revision']) and re.fullmatch('[0-9a-f]{40}', source['git_tree']), 'source fields')
    require(not source['dirty'] or development, 'development capture cannot promote')
    require(digest(source['quality_inputs']) == digest(source_provenance(deadline)),
            'current workspace source/build inputs differ')
    require(qualification['toolchain'] == source['quality_inputs']['toolchain']['rustc']
            and qualification['flags'] == {'CARGO_INCREMENTAL': os.environ.get('CARGO_INCREMENTAL', '')},
            'qualification differs from actual source/build inputs')
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
    require(digest(report['tests']) == digest({'kernel': 'passed', 'independent_model': 'passed', 'public_vm': 'passed', 'host_regressions': 'passed'}), 'executed prerequisites')
    require(report['identity_sha256'] == digest({key: value for key, value in report.items() if key != 'identity_sha256'}), 'evidence identity')


def capture(contract_path, output):
    deadline = time.monotonic() + RULES['capture_budget_seconds']
    contract, raw = load_contract(contract_path), contract_path.read_bytes()
    check_build_environment()
    check_process_scope(deadline)
    before = snapshot(deadline)
    require(not before['dirty'] or os.environ.get('TONDO_STDLIB_NET_CONF_ALLOW_DIRTY') == '1', 'clean workspace required; explicit development override only')
    logs = output.parent / 'stdlib-net-conformance-logs'
    logs.mkdir(parents=True, exist_ok=True)
    def run(name, arguments):
        remaining = deadline - time.monotonic()
        require(remaining > 0, 'capture time budget exhausted')
        try:
            process = subprocess.run(['cargo', *arguments, '--locked'], cwd=ROOT,
                                     capture_output=True, timeout=min(180, remaining))
        except subprocess.TimeoutExpired as error:
            (logs / (name + '.stdout')).write_bytes(error.stdout or b'')
            (logs / (name + '.stderr')).write_bytes(error.stderr or b'')
            raise ValueError(name + ' exceeded its time budget; see retained partial logs') from error
        (logs / (name + '.stdout')).write_bytes(process.stdout)
        (logs / (name + '.stderr')).write_bytes(process.stderr)
        require(process.returncode == 0, name + ' failed; see retained logs')
        return process.stdout.decode()
    vm = run('vm', ['run', '-q', '-p', 'tondo-reliability', '--example', 'net_conformance_vm'])
    native = run('native', ['run', '-q', '-p', 'tondo-native-runtime', '--example', 'net_conformance'])
    comparison = compare(vm, native)
    run('kernel', ['test', '-q', '-p', 'tondo-stdlib', '--lib', 'net::tests'])
    run('model', ['test', '-q', '-p', 'tondo-reliability', '--lib', 'net_model'])
    run('model-kernel', ['test', '-q', '-p', 'tondo-reliability', '--test', 'net_kernel_models'])
    run('retained-corpus', ['test', '-q', '-p', 'tondo-reliability', '--test', 'net_corpus'])
    run('hosted-model', ['test', '-q', '-p', 'tondo-reliability', '--test', 'net_hosted_models'])
    run('public-vm', ['test', '-q', '-p', 'tondo-reliability', '--example', 'net_conformance_vm'])
    run('host', ['test', '-q', '-p', 'tondo-compiler', '--lib', 'net_'])
    run('native-reference', ['test', '-q', '-p', 'tondo-native-runtime', '--example', 'net_conformance'])
    require(snapshot(deadline) == before and contract_path.read_bytes() == raw, 'capture inputs changed')
    toolchain = subprocess.check_output(['rustc', '-vV'], cwd=ROOT).decode().strip()
    target = next(line.removeprefix('host: ') for line in toolchain.splitlines() if line.startswith('host: '))
    report = {'format': 'tondo-stdlib-net-conformance-evidence/1', 'task': 'STD-NET-CONF-001', 'status': 'passed',
              'qualification': {'target': target, 'toolchain': toolchain, 'profile': 'dev', 'flags': {'CARGO_INCREMENTAL': os.environ.get('CARGO_INCREMENTAL', '')}},
              'source': before, 'contract_sha256': sha(raw), 'boundary': BOUNDARY, 'comparison': comparison,
              'tests': {'kernel': 'passed', 'independent_model': 'passed', 'public_vm': 'passed', 'host_regressions': 'passed'}}
    report['identity_sha256'] = digest(report)
    validate_report(report, contract, raw, before['dirty'], deadline)
    require(time.monotonic() < deadline, 'capture time budget exhausted')
    temporary = output.with_suffix(output.suffix + '.tmp')
    temporary.write_text(json.dumps(report, indent=2) + '\n')
    os.replace(temporary, output)
    print('std.net conformance: OK (4 shared groups; 41 common and 15 VM-only observations; 6 capability checks; ' + ('development' if before['dirty'] else 'clean-source') + ')')


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
        validate_report(load_json(arguments.report.read_bytes()), contract, arguments.contract.read_bytes(), arguments.allow_development)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, TypeError, KeyError, OSError, subprocess.SubprocessError) as error:
        print('std.net conformance: ' + str(error), file=sys.stderr)
        sys.exit(1)
