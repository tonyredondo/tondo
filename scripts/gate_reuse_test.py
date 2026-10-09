"""Current-run reuse must never accept an old, incomplete or altered producer."""

import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import gate_reuse as reuse
import test_gate_partitions as gate


class CurrentRunReuseTests(unittest.TestCase):
    def fixture(self, root):
        evidence = root / 'evidence'
        source = {'fixture': True, 'run': {'id': 'this-run', 'attempt': '1'}}
        receipts = {}
        for partition in gate.WORKERS:
            entries = []
            for name in gate.plans()[partition]:
                path = evidence / 'logs' / (name + '.log')
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture passed: ' + name + '\n')
                entries.append({'name': name, 'log_sha256': gate.digest(path)})
            artifacts = {}
            if partition == 'foundation':
                for name in gate.FOUNDATION_ARTIFACTS:
                    path = evidence / name
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(json.dumps({'format': 'tondo-conformance-result-draft/2', 'passed': True}))
                    artifacts[name] = gate.digest(path)
            value = {'format': gate.FORMAT, 'partition': partition, 'status': 'passed',
                     'source': copy.deepcopy(source), 'steps': entries, 'artifacts': artifacts}
            path = evidence / 'gate-partitions' / (partition + '.json')
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(json.dumps(value))
            receipts[partition] = path
        contract = json.loads((gate.ROOT/'testing/stdlib-conformance.json').read_text())
        return evidence, source, contract, receipts

    def test_all_owners_and_corpus_have_exact_current_producers(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence, source, contract, _ = self.fixture(Path(directory))
            result = reuse.collect(evidence, contract, source)
            self.assertEqual(result['source'], source)
            for owner in contract['owners']:
                self.assertIn('owner-' + owner['id'].removeprefix('std.'), result['commands'])
            self.assertEqual(result['commands']['draft-run']['producer'], 'conformance-run')
            self.assertIn('tondo-reference-adapter', result['commands']['draft-adapter-build']['command'])
            # A final-only command has no completed worker observation to reuse.
            final_command = 'scripts/stdlib-codec-conformance.sh'
            identity = 'case-command-' + reuse.hashlib.sha256(final_command.encode()).hexdigest()[:12]
            self.assertNotIn(identity, result['commands'])

    def test_old_run_missing_worker_failure_or_changed_logs_are_refused(self):
        for mutation in ('run', 'attempt', 'missing', 'failed', 'log', 'output', 'bindings'):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                evidence, source, contract, receipts = self.fixture(Path(directory))
                path = receipts['foundation']
                value = json.loads(path.read_text())
                if mutation == 'missing': path.unlink()
                elif mutation == 'log': (evidence/'logs/test.log').write_text('edited')
                elif mutation == 'output': (evidence/'conformance-result.json').write_text('{}')
                else:
                    if mutation == 'run': value['source']['run']['id'] = 'previous-run'
                    elif mutation == 'attempt': value['source']['run']['attempt'] = '2'
                    elif mutation == 'failed': value['status'] = 'failed'
                    elif mutation == 'bindings': value['artifacts'].pop('conformance-result.json')
                    path.write_text(json.dumps(value))
                with self.assertRaises((ValueError, OSError)):
                    reuse.collect(evidence, contract, source)

    def test_missing_run_and_nonexact_owner_arguments_do_not_fall_back(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence, source, contract, _ = self.fixture(Path(directory))
            with self.assertRaises(ValueError): reuse.collect(evidence, contract, {'fixture': True})
            contract['owners'][0]['owner_command'] += ' --different-input'
            with self.assertRaises(ValueError): reuse.collect(evidence, contract, source)

    def test_different_corpus_inputs_are_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence, source, contract, _ = self.fixture(Path(directory))
            contract['runner']['run_command'] = contract['runner']['run_command'].replace('--lineage draft', '--lineage released')
            with self.assertRaisesRegex(ValueError, 'corpus execution inputs differ'):
                reuse.collect(evidence, contract, source)

    def test_corrupt_corpus_is_refused_even_with_an_updated_output_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            evidence, source, contract, receipts = self.fixture(Path(directory))
            output = evidence/'conformance-result.json'
            output.write_text('{"format":"tondo-conformance-result-draft/2","passed":false}')
            path = receipts['foundation']
            receipt = json.loads(path.read_text())
            receipt['artifacts']['conformance-result.json'] = gate.digest(output)
            path.write_text(json.dumps(receipt))
            with self.assertRaises(ValueError): reuse.collect(evidence, contract, source)


if __name__ == '__main__':
    unittest.main()
