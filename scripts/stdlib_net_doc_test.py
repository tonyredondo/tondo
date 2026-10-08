"""Bind the networking guide to its exercised project and reject documentary drift."""
from pathlib import Path
import importlib.util
import unittest
import tempfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location('net_doc_check', ROOT/'scripts/stdlib_net_doc.py')
checker = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checker)


class GuideChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.document = checker.read(ROOT/'docs/contracts/stdlib-net.md')
        cls.source = checker.read(ROOT/'acceptance/projects/net-usage/src/main.to')
        cls.project = checker.read(ROOT/'acceptance/projects/net-usage/tondo.toml')

    def check(self, document=None, source=None, project=None):
        checker.check(self.document if document is None else document,
                      self.source if source is None else source,
                      self.project if project is None else project)

    def test_exact_exercised_project_is_accepted(self):
        self.check()

    def test_every_example_fragment_is_bound(self):
        for marker in [*checker.EXAMPLES, 'entrypoint']:
            with self.subTest(marker=marker), self.assertRaises(ValueError):
                self.check(document=self.document.replace(
                    '<!-- net-doc:'+marker+' -->', '<!-- net-doc:missing -->'))

    def test_matching_edits_cannot_skip_an_example(self):
        for function in checker.EXAMPLES.values():
            call='    '+function+'()?\n'
            with self.subTest(function=function), self.assertRaisesRegex(ValueError, 'entrypoint'):
                self.check(document=self.document.replace(call, ''),
                           source=self.source.replace(call, ''))

    def test_duplicate_or_unbound_guide_code_is_rejected(self):
        for suffix in [self.document, '\n~~~tondo\nfn hidden() {}\n~~~\n',
                       '\n~~~toml\n[hidden]\n~~~\n']:
            with self.subTest(suffix=suffix[:24]), self.assertRaises(ValueError):
                self.check(document=self.document+suffix)

    def test_source_and_document_drift_are_rejected(self):
        for before, after in [('    assert(count > 0)', '    assert(false)'),
                              ('import std.io\n', ''),
                              ('fn tcpRoundtrip()', 'fn uncalled()')]:
            with self.subTest(before=before), self.assertRaises(ValueError):
                self.check(document=self.document.replace(before,after))

    def test_configuration_and_resolver_identity_are_bound(self):
        for before, after in [('127.0.0.1:9', '127.0.0.1:53'),
                              ('"clock", ', ''),
                              ('name = "net_usage"', 'name = "other"')]:
            with self.subTest(before=before), self.assertRaises(ValueError):
                self.check(document=self.document.replace(before, after),
                           project=self.project.replace(before, after))

    def test_canonical_whitespace_is_required(self):
        temporary=tempfile.TemporaryDirectory(prefix='tondo-net-doc-whitespace-')
        self.addCleanup(temporary.cleanup)
        path=Path(temporary.name)/'noncanonical'
        for raw in [b'line', b'line\r\n', b'line \n']:
            path.write_bytes(raw)
            with self.subTest(raw=raw), self.assertRaises(ValueError):
                checker.read(path)


if __name__ == '__main__':
    unittest.main()
