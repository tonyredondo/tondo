"""Retained-sample laws for truthful direct Rust regex performance reports."""

import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import stdlib_regex_performance as perf


class PerformanceReports(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.contract = perf.load_contract(perf.ROOT / 'testing/stdlib-regex-performance.json')
        cls.tree = 'a' * 64
        cls.context = {
            'git_revision': 'b' * 40, 'source_tree_sha256': cls.tree,
            'toolchain': {'rustc': 'rustc 1.93.0 (fixture)', 'cargo': 'cargo 1.93.0 (fixture)'},
            'flags': {'rustflags': '', 'encoded_rustflags': '', 'incremental': '0'},
            'host': {'cpu_model': 'synthetic', 'os': 'synthetic'}, 'capture': 'development',
        }
        # Synthetic validator inputs, never benchmark observations. A large
        # final outlier must remain present in the retained distribution.
        cls.rows = [{
            'workload_id': spec['id'], 'operation': spec['operation'],
            'process': process, 'repetition': repetition,
            'nanos': 1_000_000 if (process, repetition) == (3, 8)
                else 100 * ((process - 1) * 9 + repetition + 1),
            'dispatch': spec['dispatch'], 'counters': copy.deepcopy(spec['counters']),
        } for spec in cls.contract['workloads']
          for process in range(1, 4) for repetition in range(9)]
        cls.report = perf.render(cls.contract, cls.rows, cls.context)

    def reject_rows(self, change):
        rows = copy.deepcopy(self.rows)
        change(rows)
        with self.assertRaises((ValueError, KeyError, TypeError)):
            perf.measurements(self.contract, rows)

    def reject_report(self, change):
        report = copy.deepcopy(self.report)
        change(report)
        with self.assertRaises((ValueError, KeyError, TypeError)):
            perf.validate_report(self.contract, report, self.tree)

    def test_distribution_preserves_outlier_ordinals_and_input_order_independence(self):
        for item in self.report['measurements']:
            self.assertEqual((item['median_ns'], item['p95_ns'], item['p99_ns']),
                             (1400, 2600, 1_000_000))
            self.assertEqual(item['samples'][-1], {'process': 3, 'repetition': 8, 'nanos': 1_000_000})
            self.assertEqual(item['throughput_operations_per_second'], 16_000_000_000 // 1400)
            payload = item['counters']['output_bytes'] or item['counters']['input_bytes']
            self.assertEqual(item['throughput_bytes_per_second'], payload * 16_000_000_000 // 1400)
        self.assertEqual(perf.render(self.contract, list(reversed(self.rows)), self.context), self.report)

    def test_samples_reject_missing_duplicates_invalid_numbers_and_route_changes(self):
        mutations = {
            'missing': lambda rows: rows.pop(),
            'duplicated-coordinate': lambda rows: rows[-1].update(process=3, repetition=7),
            'process-zero': lambda rows: rows[0].update(process=0),
            'process-PID': lambda rows: rows[0].update(process=12345),
            'boolean-process': lambda rows: rows[0].update(process=True),
            'boolean-repetition': lambda rows: rows[0].update(repetition=False),
            'extra-repetition': lambda rows: rows[0].update(repetition=9),
            'zero-latency': lambda rows: rows[0].update(nanos=0),
            'float-latency': lambda rows: rows[0].update(nanos=1.25),
            'boolean-latency': lambda rows: rows[0].update(nanos=True),
            'unknown-workload': lambda rows: rows[0].update(workload_id='unmeasured'),
            'native-dispatch': lambda rows: rows[0].update(dispatch='native'),
            'wrong-operation': lambda rows: rows[0].update(operation='unknown'),
            'ambient-field': lambda rows: rows[0].update(pid=12345),
            'counter-drift': lambda rows: rows[0]['counters'].update(
                allocations=rows[0]['counters']['allocations'] + 1),
        }
        for name, change in mutations.items():
            with self.subTest(name=name):
                self.reject_rows(change)

    def test_report_rejects_corrupted_metrics_identity_provenance_and_promotion(self):
        mutations = {
            'tail': lambda value: value['measurements'][0].update(p99_ns=2600),
            'throughput': lambda value: value['measurements'][0].update(throughput_operations_per_second=0),
            'identity': lambda value: value['measurements'][0].update(identity_sha256='0' * 64),
            'source': lambda value: value.update(source_tree_sha256='0' * 64),
            'probe': lambda value: value.update(probe_sha256='0' * 64),
            'revision': lambda value: value.update(git_revision='main'),
            'sample-count': lambda value: value['measurements'][0].update(sample_count=26),
            'missing-sample': lambda value: value['measurements'][0]['samples'].pop(),
            'ambient-report': lambda value: value.update(timestamp='now'),
            'ambient-sample': lambda value: value['measurements'][0]['samples'][0].update(pid=12345),
            'native-claim': lambda value: value['strategy'].update(native_aot='verified'),
            'hosted-claim': lambda value: value['strategy'].update(hosted_vm='verified'),
            'allocator-claim': lambda value: value['resource_model'].update(allocations='OS calls'),
            'wrong-toolchain': lambda value: value['toolchain'].update(rustc='rustc 1.94.0 (fixture)'),
            'malformed-toolchain': lambda value: value['toolchain'].update(cargo=123),
            'missing-flags': lambda value: value['flags'].pop('rustflags'),
            'invalid-capture': lambda value: value.update(capture='verified'),
        }
        for name, change in mutations.items():
            with self.subTest(name=name):
                self.reject_report(change)

    def test_resources_preserve_unmeasured_fields_and_fallible_prefix(self):
        for spec in self.contract['workloads']:
            mutations = {'native_live_handles': 0, 'terminal_open_cursors': 1,
                         'allocations': -1, 'logical_memory_bytes': True, 'program_states': 4097}
            if spec['operation'] in {'compile', 'replace-first', 'replace-all'}:
                mutations['matching_steps'] = 0
            else:
                mutations['matching_steps'] = None
            if spec['id'] == 'reject-matches':
                mutations['matches'] = 0
            if spec['id'] in perf.ERRORS:
                mutations.update(output_bytes=1, bytes_copied=1, adversarial_rejections=15)
            for field, value in mutations.items():
                counters = copy.deepcopy(spec['counters'])
                counters[field] = value
                with self.subTest(workload=spec['id'], field=field), self.assertRaises(ValueError):
                    perf.validate_counters(counters, spec['id'])

    def test_identity_binds_build_context_and_excludes_descriptive_host(self):
        identities = [item['identity_sha256'] for item in self.report['measurements']]
        context = copy.deepcopy(self.context)
        context['host'] = {'cpu_model': 'another CPU', 'os': 'another OS'}
        self.assertEqual([item['identity_sha256']
                          for item in perf.render(self.contract, self.rows, context)['measurements']], identities)
        for field, value in {'git_revision': 'c' * 40, 'source_tree_sha256': 'd' * 64,
                             'flags': {'rustflags': '-C debuginfo=1', 'encoded_rustflags': '', 'incremental': '0'}}.items():
            context = copy.deepcopy(self.context)
            context[field] = value
            self.assertNotEqual([item['identity_sha256']
                                 for item in perf.render(self.contract, self.rows, context)['measurements']], identities)

    def test_clean_capture_requires_actual_committed_probe_bytes(self):
        context = copy.deepcopy(self.context)
        context['capture'] = 'clean-source'
        probe = (perf.ROOT / perf.PROBE).read_bytes()
        self.assertEqual(hashlib.sha256(probe).hexdigest(), self.contract['probe']['sha256'])
        with patch.object(perf.subprocess, 'run', return_value=perf.subprocess.CompletedProcess([], 0, probe)) as call:
            report = perf.render(self.contract, self.rows, context)
            self.assertEqual(report['capture'], 'clean-source')
            self.assertEqual(call.call_args.args[0][-1], f"{context['git_revision']}:{perf.PROBE}")
        for result in [perf.subprocess.CompletedProcess([], 1, b''),
                       perf.subprocess.CompletedProcess([], 0, probe + b'changed')]:
            with patch.object(perf.subprocess, 'run', return_value=result), self.assertRaises(ValueError):
                perf.render(self.contract, self.rows, context)

    def test_contract_pins_workloads_protocol_resource_semantics_and_probe(self):
        mutations = {
            'lost-samples': lambda value: value['protocol'].update(independent_processes=1),
            'native-promotion': lambda value: value['strategy'].update(native_runtime_abi='verified'),
            'unrecorded-profile': lambda value: value.update(profile='release'),
            'stale-probe': lambda value: value['probe'].update(sha256='0' * 64),
            'RSS-claim': lambda value: value['resource_model'].update(logical_memory='RSS'),
            'allocator-claim': lambda value: value['resource_model'].update(allocations='OS calls'),
            'hidden-exclusion': lambda value: value['resource_model']['excluded'].pop(),
            'duplicate-metric': lambda value: value['metrics'].append(value['metrics'][0]),
            'missing-workload': lambda value: value['workloads'].pop(),
            'wrong-error': lambda value: value['workloads'][-1].update(expected_error=None),
        }
        with tempfile.TemporaryDirectory(prefix='tondo-regex-report-test-') as folder:
            candidate = Path(folder) / 'contract.json'
            for name, change in mutations.items():
                value = copy.deepcopy(self.contract)
                change(value)
                candidate.write_text(json.dumps(value, indent=2) + '\n')
                with self.subTest(name=name), self.assertRaises((ValueError, KeyError, TypeError)):
                    perf.load_contract(candidate)
            for suffix in ['', '\n\n', '\r\n', ' \n']:
                candidate.write_text(json.dumps(self.contract) + suffix)
                with self.subTest(suffix=repr(suffix)), self.assertRaises(ValueError):
                    perf.load_contract(candidate)


if __name__ == '__main__':
    unittest.main()
