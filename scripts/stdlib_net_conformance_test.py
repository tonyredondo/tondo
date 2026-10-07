"""Report, lifecycle and provenance laws for the NET conformance boundary."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

import stdlib_net_conformance as c


class ConformanceLaws(unittest.TestCase):
    def setUp(self):
        self.path = c.ROOT / c.CONTRACT
        self.raw = self.path.read_bytes()
        self.contract = c.load_contract(self.path)
        expected = c.expected_observations()
        self.vm = [{'id': key, 'observations': lines, 'envelopes_closed': True} for key, lines in expected.items()]
        self.vm += [{'id': key, 'observations': lines, 'envelopes_closed': True} for key, lines in c.VM_ONLY.items()]
        self.vm.append({'id': 'vm-capabilities', 'observations': c.expected_capabilities()})
        self.native = [{'id': key, 'observations': lines, 'live_runtime_objects': 0} for key, lines in expected.items()]

    @staticmethod
    def wire(values):
        return ''.join(json.dumps(value) + '\n' for value in values)

    @staticmethod
    def identify(report):
        report['identity_sha256'] = c.digest({key: value for key, value in report.items() if key != 'identity_sha256'})
        return report

    def report(self):
        source = c.snapshot()
        return self.identify({
            'format': 'tondo-stdlib-net-conformance-evidence/1', 'task': 'STD-NET-CONF-001',
            'status': 'passed', 'qualification': {'target': 'x86_64-unknown-linux-gnu',
                'toolchain': source['quality_inputs']['toolchain']['rustc'], 'profile': 'dev',
                'flags': {'CARGO_INCREMENTAL': c.os.environ.get('CARGO_INCREMENTAL', '')}},
            'source': source, 'contract_sha256': c.sha(self.raw), 'boundary': c.BOUNDARY,
            'comparison': c.compare(self.wire(self.vm), self.wire(self.native)),
            'tests': {'kernel': 'passed', 'independent_model': 'passed', 'public_vm': 'passed', 'host_regressions': 'passed'},
        })

    def test_complete_ordered_actual_values_are_required(self):
        for broken in [self.vm[:-1], self.vm + [self.vm[0]], self.vm[::-1]]:
            with self.assertRaises(ValueError):
                c.compare(self.wire(broken), self.wire(self.native))
        for field, value in [('id', 'wrong'), ('envelopes_closed', False),
                             ('envelopes_closed', 1), ('extra', 'unrecorded')]:
            broken = copy.deepcopy(self.vm)
            broken[0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                c.compare(self.wire(broken), self.wire(self.native))
        # Agreement between two altered outputs cannot replace an exact oracle.
        vm, native = copy.deepcopy(self.vm), copy.deepcopy(self.native)
        vm[0]['observations'][0] = native[0]['observations'][0] = 'same-but-wrong'
        with self.assertRaises(ValueError):
            c.compare(self.wire(vm), self.wire(native))
        for value in [False, 0.0, 1]:
            native = copy.deepcopy(self.native)
            native[0]['live_runtime_objects'] = value
            with self.subTest(counter=value), self.assertRaises(ValueError):
                c.compare(self.wire(self.vm), self.wire(native))

    def test_capability_records_are_not_passing_labels(self):
        for field, value in [('result', 'accepted'), ('network', True),
                             ('network', 0), ('form', 'unchecked')]:
            broken = copy.deepcopy(self.vm)
            broken[-1]['observations'][0][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                c.compare(self.wire(broken), self.wire(self.native))
        broken = copy.deepcopy(self.vm)
        broken[-1]['observations'].pop()
        with self.assertRaises(ValueError):
            c.compare(self.wire(broken), self.wire(self.native))

    def test_vm_only_integration_cannot_be_claimed_by_native_reference(self):
        for key in c.VM_ONLY:
            broken = copy.deepcopy(self.vm)
            row = next(row for row in broken if row['id'] == key)
            row['observations'][0] = 'unchecked'
            with self.subTest(case=key), self.assertRaises(ValueError):
                c.compare(self.wire(broken), self.wire(self.native))
        with self.assertRaises(ValueError):
            c.compare(self.wire(self.vm), self.wire(self.native + [self.vm[4]]))
        with self.assertRaises(ValueError):
            c.load_json('{"result":"unchecked","result":"accepted"}')

    def test_contract_states_preserve_the_narrow_runtime_boundary(self):
        with tempfile.TemporaryDirectory(prefix='tondo-net-conf-laws-') as directory:
            path = Path(directory) / 'contract.json'
            for status in ['adapter-ready', c.STATUS]:
                valid = copy.deepcopy(self.contract)
                valid['status'] = status
                path.write_text(json.dumps(valid) + '\n')
                c.load_contract(path)
            for field, value in [('sources', []), ('boundary', {}), ('rules', {}),
                                 ('cases', self.contract['cases'][::-1]), ('next_blocks', []),
                                 ('status', 'published'), ('extra', True)]:
                broken = copy.deepcopy(self.contract)
                broken[field] = value
                path.write_text(json.dumps(broken) + '\n')
                with self.subTest(field=field), self.assertRaises(ValueError):
                    c.load_contract(path)
            for field, value in [('full_reference_vectors', 40.0),
                                 ('common_observations', True),
                                 ('fresh_process_per_adapter', 1),
                                 ('vm_only_observations', 0)]:
                broken = copy.deepcopy(self.contract)
                broken['rules'][field] = value
                path.write_text(json.dumps(broken) + '\n')
                with self.subTest(rule=field), self.assertRaises(ValueError):
                    c.load_contract(path)
            broken = copy.deepcopy(self.contract)
            broken['boundary']['native_aot'] = 'verified'
            path.write_text(json.dumps(broken) + '\n')
            with self.assertRaises(ValueError):
                c.load_contract(path)
            path.write_bytes(self.raw.replace(b'\n', b'\r\n'))
            with self.assertRaises(ValueError):
                c.load_contract(path)

    def test_report_identity_source_flags_and_prerequisites_are_bound(self):
        report = self.report()
        c.validate_report(report, self.contract, self.raw, True)
        self.assertEqual(report['identity_sha256'], self.report()['identity_sha256'])
        for section, field, value in [
            ('qualification', 'target', 'aarch64-unknown-linux-gnu'),
            ('qualification', 'profile', 'release'),
            ('qualification', 'toolchain', 93),
            ('qualification', 'toolchain', 'rustc 1.93.0 forged'),
            ('qualification', 'flags', {'CARGO_INCREMENTAL': '0', 'RUSTFLAGS': '-C opt-level=3'}),
            ('tests', 'kernel', 'skipped'), ('tests', 'independent_model', 'pending'),
            ('tests', 'public_vm', 'private-host-callable'), ('tests', 'host_regressions', 'pending'),
            ('boundary', 'native_abi', 'verified'), ('source', 'git_tree', '0'*40),
            ('source', 'revision', 0), ('source', 'sources', []),
        ]:
            broken = copy.deepcopy(report)
            broken[section][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                c.validate_report(self.identify(broken), self.contract, self.raw, True)
        broken = copy.deepcopy(report)
        broken['source']['sources'][c.SOURCES[0]] = 'sha256:' + '0'*64
        with self.assertRaises(ValueError):
            c.validate_report(self.identify(broken), self.contract, self.raw, True)
        for field in ['pid', 'timestamp', 'artifact_path', 'address']:
            broken = copy.deepcopy(report)
            broken[field] = 'ambient'
            with self.subTest(field=field), self.assertRaises(ValueError):
                c.validate_report(self.identify(broken), self.contract, self.raw, True)

    def test_development_and_unknown_executable_inputs_cannot_promote(self):
        report = self.report()
        report['source']['dirty'] = True
        self.identify(report)
        with self.assertRaises(ValueError):
            c.validate_report(report, self.contract, self.raw)
        c.metadata_only(['TONDO_IMPLEMENTATION_TRACKER.md', 'testing/conformance-ratchet.json'])
        for path in ['Cargo.lock', '.cargo/config.toml', 'scripts/probe.py',
                     'crates/tondo-vm/src/runtime/execute.rs', 'crates/tondo-compiler/src/untracked.rs']:
            with self.subTest(path=path), self.assertRaises(ValueError):
                c.metadata_only([path])
        # The pre-introduction revision lacks the committed probes even when
        # a caller changes the dirty flag and recomputes the report identity.
        old = c.git('log', '--diff-filter=A', '--format=%H', '--', c.SOURCES[0]).decode().splitlines()
        revision = old[-1] + '^' if old else 'HEAD'
        report['source']['revision'] = c.git('rev-parse', revision).decode().strip()
        report['source']['git_tree'] = c.git('rev-parse', revision + '^{tree}').decode().strip()
        report['source']['dirty'] = False
        with self.assertRaises((ValueError, subprocess.CalledProcessError)):
            c.validate_report(self.identify(report), self.contract, self.raw)

    def test_failed_probe_retains_logs_and_never_replaces_existing_report(self):
        with tempfile.TemporaryDirectory(prefix='tondo-net-conf-failure-') as directory:
            output = Path(directory) / 'report.json'
            output.write_text('old complete report\n')
            source = c.snapshot()
            source['dirty'] = True
            failed = subprocess.CompletedProcess([], 7, b'partial stdout\n', b'probe refused\n')
            with mock.patch.dict(c.os.environ, {'TONDO_STDLIB_NET_CONF_ALLOW_DIRTY': '1', 'TONDO_TEST_PROCESS_CGROUP': '/test-fixture'}, clear=True), \
                 mock.patch.object(c, 'snapshot', return_value=source), \
                 mock.patch.object(c, 'check_process_scope'), \
                 mock.patch.object(c.subprocess, 'run', return_value=failed):
                with self.assertRaisesRegex(ValueError, 'vm failed'):
                    c.capture(self.path, output)
            self.assertEqual(output.read_text(), 'old complete report\n')
            self.assertEqual((output.parent / 'stdlib-net-conformance-logs/vm.stderr').read_bytes(), b'probe refused\n')
            self.assertFalse(output.with_suffix('.json.tmp').exists())

    def test_expired_capture_budget_starts_no_more_children(self):
        with tempfile.TemporaryDirectory(prefix='tondo-net-conf-budget-') as directory:
            output = Path(directory) / 'report.json'
            output.write_text('old complete report\n')
            source = c.snapshot()
            source['dirty'] = True
            with mock.patch.dict(c.os.environ, {'TONDO_STDLIB_NET_CONF_ALLOW_DIRTY': '1', 'TONDO_TEST_PROCESS_CGROUP': '/test-fixture'}, clear=True), \
                 mock.patch.object(c, 'snapshot', return_value=source), \
                 mock.patch.object(c, 'check_process_scope'), \
                 mock.patch.object(c.time, 'monotonic', side_effect=[0, 481]), \
                 mock.patch.object(c.subprocess, 'run') as command:
                with self.assertRaisesRegex(ValueError, 'capture time budget exhausted'):
                    c.capture(self.path, output)
                command.assert_not_called()
            self.assertEqual(output.read_text(), 'old complete report\n')

    def test_timed_out_child_retains_partial_logs_without_report_replacement(self):
        with tempfile.TemporaryDirectory(prefix='tondo-net-conf-timeout-') as directory:
            output = Path(directory) / 'report.json'
            output.write_text('old complete report\n')
            source = c.snapshot()
            source['dirty'] = True
            expired = subprocess.TimeoutExpired(['cargo'], 1, b'partial\n', b'child pending\n')
            with mock.patch.dict(c.os.environ, {'TONDO_STDLIB_NET_CONF_ALLOW_DIRTY': '1', 'TONDO_TEST_PROCESS_CGROUP': '/test-fixture'}, clear=True), \
                 mock.patch.object(c, 'snapshot', return_value=source), \
                 mock.patch.object(c, 'check_process_scope'), \
                 mock.patch.object(c.subprocess, 'run', side_effect=expired):
                with self.assertRaisesRegex(ValueError, 'vm exceeded its time budget'):
                    c.capture(self.path, output)
            self.assertEqual(output.read_text(), 'old complete report\n')
            self.assertEqual((output.parent / 'stdlib-net-conformance-logs/vm.stdout').read_bytes(), b'partial\n')
            self.assertEqual((output.parent / 'stdlib-net-conformance-logs/vm.stderr').read_bytes(), b'child pending\n')


if __name__ == '__main__':
    unittest.main()
