import copy
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import rust_suites as suites


class SuiteProtocolTests(unittest.TestCase):
    def test_schedule_preserves_every_suite_once_and_balances_measured_durations(self):
        values = [{'id': name} for name in 'abcdefgh']
        weights = dict(zip('abcdefgh', [90, 80, 70, 60, 30, 20, 10, 1]))
        plan = suites.schedule(values, weights, 4)
        self.assertEqual(sorted(v['id'] for shard in plan for v in shard), list('abcdefgh'))
        self.assertLessEqual(max(sum(weights[v['id']] for v in shard) for shard in plan), 100)
        self.assertEqual(plan, suites.schedule(list(reversed(values)), weights, 4))
        for invalid in (0, 5, True):
            with self.assertRaises(ValueError): suites.schedule(values, weights, invalid)
        for invalid in (-1, float('nan'), float('inf'), True):
            with self.assertRaises(ValueError): suites.schedule(values, {'a': invalid}, 4)
        with self.assertRaises(ValueError): suites.schedule([values[0], values[0]], weights, 4)

    def test_child_environment_preserves_cargo_paths_and_bounds_threads(self):
        suite = {'directory': '/fixture/package', 'package': 'fixture',
                 'binaries': {'fixture-tool': '/fixture/target/debug/fixture-tool'}}
        with patch.dict(os.environ, {'RUST_TEST_THREADS': '4'}):
            result = suites.environment(suite)
            self.assertEqual(result['CARGO_MANIFEST_DIR'], suite['directory'])
            self.assertEqual(result['CARGO_PKG_NAME'], suite['package'])
            self.assertEqual(result['CARGO_BIN_EXE_fixture-tool'], suite['binaries']['fixture-tool'])
            self.assertEqual(result['RUST_TEST_THREADS'], '4')
        for invalid in ('0', '5', 'unbounded'):
            with patch.dict(os.environ, {'RUST_TEST_THREADS': invalid}), self.assertRaises(ValueError):
                suites.environment(suite)

    def output(self, passed=2, failed=0, ignored=0, filtered=0):
        return ('test module::one ... ok\ntest module::two ... ok\n'
                f'test result: ok. {passed} passed; {failed} failed; {ignored} ignored; 0 measured; {filtered} filtered out; finished in 0.01s\n')

    def test_exact_test_identity_and_terminal_result_are_required(self):
        suite = {'tests': ['module::one', 'module::two']}
        self.assertEqual(suites.check_result(suite, self.output(), 0), 2)
        for output, code in ((self.output(), 1), (self.output(passed=1), 0),
                             (self.output(failed=1), 0), (self.output(ignored=1), 0),
                             (self.output(filtered=1), 0), (self.output()+self.output(), 0),
                             (self.output().replace('module::two', 'module::other'), 0)):
            with self.assertRaises(ValueError): suites.check_result(suite, output, code)
        self.assertEqual(suites.test_names('module::two: test\nmodule::one: test\n'), suite['tests'])
        for listing in ('one: test\none: test\n', 'one: benchmark\n'):
            with self.assertRaises(ValueError): suites.test_names(listing)

    def test_expected_panic_annotation_preserves_the_discovered_test_identity(self):
        suite = {'tests': ['module::one', 'module::two']}
        output = self.output().replace('module::two ... ok', 'module::two - should panic ... ok')
        self.assertEqual(suites.check_result(suite, output, 0), 2)
        with self.assertRaises(ValueError):
            suites.check_result(suite, output.replace('should panic', 'other annotation'), 0)

    def test_merge_refuses_missing_duplicate_failed_or_changed_suite_logs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            log = root/'suite.log'
            log.write_text(self.output())
            suite = {'id': 'one', 'tests': ['module::one', 'module::two']}
            row = {'id': 'one', 'status': 'passed', 'passed': 2, 'log': 'suite.log',
                   'log_sha256': suites.digest(log)}
            suites.verify([suite], [row], root)
            for rows in ([], [row, row], [{**row, 'status': 'failed'}], [{**row, 'passed': 1}]):
                with self.assertRaises(ValueError): suites.verify([suite], rows, root)
            log.write_text(self.output().replace('two', 'other'))
            with self.assertRaises(ValueError): suites.verify([suite], [row], root)

    def test_cargo_discovery_requires_every_workspace_target_and_a_successful_build(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(suites, 'ROOT', Path(directory)):
            root = Path(directory)
            executable = root/'target/debug/deps/fixture'
            executable.parent.mkdir(parents=True)
            executable.write_bytes(b'fixture')
            target = {'src_path': str(root/'crates/fixture/src/lib.rs'), 'kind': ['lib'], 'name': 'fixture'}
            metadata = {'workspace_members': ['fixture'], 'target_directory': str(root/'target'),
                        'packages': [{'id': 'fixture', 'name': 'fixture', 'manifest_path': str(root/'crates/fixture/Cargo.toml'),
                                      'targets': [target]}]}
            artifact = {'reason': 'compiler-artifact', 'package_id': 'fixture', 'target': target,
                        'profile': {'test': True}, 'executable': str(executable)}
            done = {'reason': 'build-finished', 'success': True}
            self.assertEqual(len(suites.discover(metadata, [artifact, done])), 1)
            for messages in ([done], [artifact], [artifact, artifact, done], [artifact, {**done, 'success': False}]):
                with self.assertRaises(ValueError): suites.discover(metadata, messages)


if __name__ == '__main__':
    unittest.main()
