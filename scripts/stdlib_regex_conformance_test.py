#!/usr/bin/env python3
"""Report-law negatives independent of Cargo/probe execution."""

import copy
import json
import pathlib
import tempfile
import unittest

import stdlib_regex_conformance as c


class ConformanceLaws(unittest.TestCase):
    def setUp(self):
        self.path = c.ROOT / c.CONTRACT
        self.raw = self.path.read_bytes()
        self.contract = c.load_contract(self.path)
        self.vm = "\n".join(c.LINES.values()) + "\nregex-conformance-ok\n"
        native = [{"id": key, "status": "passed", "line": line, "cleanup": True,
                   "live_runtime_objects": 0} for key, line in c.LINES.items()]
        native += [{"id": "regex-conformance", "status": "passed"}]
        self.native = "\n".join(json.dumps(x) for x in native) + "\n"

    def report(self):
        report = {"format": "tondo-stdlib-regex-conformance-evidence/1", "task": "STD-REGEX-CONF-001",
                  "status": "passed", "qualification": {"target": "x86_64-unknown-linux-gnu",
                  "toolchain": "rustc 1.93.0 test", "profile": "dev", "flags": {"CARGO_INCREMENTAL": "0"}},
                  "source": c.snapshot(), "contract_sha256": c.sha(self.raw), "boundary": c.BOUNDARY,
                  "comparison": c.compare(self.vm, self.native),
                  "tests": {"kernel": "passed", "independent_model": "passed", "vm_dispatch": "passed"}}
        return self.identify(report)

    @staticmethod
    def identify(report):
        report["identity_sha256"] = c.digest({key: value for key, value in report.items() if key != "identity_sha256"})
        return report

    def test_exact_order_and_native_fields(self):
        comparison = c.compare(self.vm, self.native)
        self.assertEqual(len(comparison["native_cases"]), 8)
        records = comparison["native_cases"]
        for mutation in [records[:-1], records + [records[0]], records[::-1]]:
            with self.assertRaises(ValueError):
                c.compare(self.vm, "\n".join(json.dumps(x) for x in mutation))
        for key, value in [("line", "wrong"), ("cleanup", False), ("cleanup", 1),
                           ("live_runtime_objects", 1), ("live_runtime_objects", False),
                           ("status", "skipped"), ("extra", "unrecorded")]:
            mutated = copy.deepcopy(records)
            mutated[0][key] = value
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                c.compare(self.vm, "\n".join(json.dumps(x) for x in mutated))
        for text in [self.vm.replace("atomic-output", "partial-output"),
                     self.vm.replace(":41:33:", ":40:33:"), self.vm + "extra\n",
                     self.vm.replace("regex-conformance-ok\n", "")]:
            with self.assertRaises(ValueError):
                c.compare(text, self.native)

    def test_contract_mutations_and_both_states(self):
        with tempfile.TemporaryDirectory(prefix="tondo-regex-conf-law-") as directory:
            path = pathlib.Path(directory) / "contract.json"
            for status in ["adapter-ready", c.STATUS]:
                valid = copy.deepcopy(self.contract)
                valid["status"] = status
                path.write_text(json.dumps(valid) + "\n")
                c.load_contract(path)
            mutations = [("status", "open"), ("sources", []), ("cases", self.contract["cases"][:-1]),
                         ("cases", self.contract["cases"][::-1]), ("boundary", {}), ("rules", {}),
                         ("report", "../../outside.json"), ("next_blocks", []), ("extra", True)]
            for key, value in mutations:
                invalid = copy.deepcopy(self.contract)
                invalid[key] = value
                path.write_text(json.dumps(invalid) + "\n")
                with self.subTest(key=key), self.assertRaises(ValueError):
                    c.load_contract(path)
            # Python equality treats True as 1 and False as 0. The exact
            # register must retain JSON boolean/integer identities as well.
            for key, value in [("same_compiled_fixture_bytes", 1),
                               ("iterator_prior_valid_matches", True),
                               ("empty_scalar_offsets", [False, 2, 6]),
                               ("valid_vectors", 41.0)]:
                invalid = copy.deepcopy(self.contract)
                invalid["rules"][key] = value
                path.write_text(json.dumps(invalid) + "\n")
                with self.subTest(rule=key), self.assertRaises(ValueError):
                    c.load_contract(path)
            path.write_bytes(self.raw.replace(b"\n", b"\r\n"))
            with self.assertRaises(ValueError):
                c.load_contract(path)

    def test_report_identity_determinism_and_ambient_fields(self):
        report = self.report()
        c.validate_report(report, self.contract, self.raw, allow_development=True)
        self.assertEqual(report["identity_sha256"], self.report()["identity_sha256"])
        for extra in ["timestamp", "pid", "artifact_path", "address"]:
            invalid = copy.deepcopy(report)
            invalid[extra] = "ambient"
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                c.validate_report(self.identify(invalid), self.contract, self.raw, True)

    def test_source_build_and_prerequisite_bindings(self):
        c.require_metadata_only_changes([])
        c.require_metadata_only_changes(["TONDO_IMPLEMENTATION_TRACKER.md",
                                        "docs/contracts/stdlib-regex.md",
                                        "testing/conformance-ratchet.json"])
        for path in ["crates/tondo-vm/src/runtime/execute.rs", "Cargo.lock",
                     "scripts/stdlib_regex_conformance.py", ".cargo/config.toml",
                     "testing/fixture.rs", "docs/example.sh"]:
            with self.subTest(changed=path), self.assertRaises(ValueError):
                c.require_metadata_only_changes([path])
        report = self.report()
        variants = []
        invalid = copy.deepcopy(report)
        invalid["source"]["sources"][c.SHARED] = "sha256:" + "0" * 64
        variants.append(invalid)
        invalid = copy.deepcopy(report)
        invalid["source"]["git_tree"] = "0" * 40
        variants.append(invalid)
        invalid = copy.deepcopy(report)
        invalid["contract_sha256"] = "sha256:" + "0" * 64
        variants.append(invalid)
        for section, key, value in [("qualification", "profile", "release"),
                                    ("qualification", "flags", {"RUSTFLAGS": "unrecorded"}),
                                    ("tests", "kernel", "skipped"), ("tests", "independent_model", "pending"),
                                    ("boundary", "native_aot", "verified"),
                                    ("comparison", "native_log_sha256", "sha256:" + "0" * 64)]:
            invalid = copy.deepcopy(report)
            invalid[section][key] = value
            variants.append(invalid)
        for invalid in variants:
            with self.assertRaises(ValueError):
                c.validate_report(self.identify(invalid), self.contract, self.raw, True)

    def test_dirty_evidence_and_committed_source_rejection(self):
        report = self.report()
        report["source"]["dirty"] = True
        self.identify(report)
        with self.assertRaises(ValueError):
            c.validate_report(report, self.contract, self.raw)
        c.validate_report(report, self.contract, self.raw, True)
        report["source"]["dirty"] = False
        self.identify(report)
        # Always select the revision before probe introduction, even after
        # metadata-only commits. HEAD~1 can legitimately contain identical code.
        introductions = c.git("log", "--diff-filter=A", "--format=%H", "--", c.VM).decode().splitlines()
        old_ref = introductions[-1] + "^" if introductions else "HEAD"
        report["source"]["revision"] = c.git("rev-parse", old_ref).decode().strip()
        report["source"]["git_tree"] = c.git("rev-parse", old_ref + "^{tree}").decode().strip()
        self.identify(report)
        with self.assertRaises((ValueError, c.subprocess.CalledProcessError)):
            c.validate_report(report, self.contract, self.raw)


    def test_historical_owner_progression_rejects_early_and_false_claims(self):
        parent = json.loads((c.ROOT / 'testing/stdlib-regex.json').read_bytes())
        parent.pop('conformance', None)
        parent.pop('documentation', None)
        program = 'include "stdlib_regex_progression"; regex_kernel_progression'
        leaves = ['STD-REGEX-IMPL-001', 'STD-REGEX-TEST-001', 'STD-REGEX-PERF-001', 'STD-REGEX-CONF-001', 'STD-REGEX-DOC-001']
        design = ['allocation', 'claims_before_perf_gate', 'dispatch', 'matching', 'parser_stack', 'scalar_oracle', 'simd_allowed_after_equivalence']

        def accepts(value):
            result = c.subprocess.run(['jq', '-e', '-L', str(c.ROOT / 'scripts'), program], input=json.dumps(value), text=True, capture_output=True)
            if result.returncode not in [0, 1]:
                raise AssertionError(result.stderr)
            return result.returncode == 0

        states = {}
        for stage in ['impl-ready', 'impl-verified', 'test-verified', 'perf-ready', 'perf-verified', 'conf-ready', 'conf-verified']:
            value = copy.deepcopy(parent)
            if stage.startswith('impl-'):
                value.pop('testing_contract')
            if stage in ['impl-ready', 'impl-verified', 'test-verified']:
                value['performance'] = {key: value['performance'][key] for key in design}
            if stage == 'impl-ready':
                value['implementation']['status'] = 'ready-stdlib-kernel'
                value['implementation']['quality_gate'] = 'pending-80-percent-per-scope'
                next_index = 0
            elif stage == 'impl-verified':
                next_index = 1
            elif stage in ['test-verified', 'perf-ready']:
                next_index = 2
                if stage == 'perf-ready':
                    value['performance']['status'] = 'measurement-ready'
            elif stage in ['perf-verified', 'conf-ready']:
                next_index = 3
            else:
                next_index = 4
            if stage.startswith('conf-'):
                value['conformance'] = dict(contract='testing/stdlib-regex-conformance.json',
                    document='docs/contracts/stdlib-regex-conformance.md', task='STD-REGEX-CONF-001',
                    hosted_vm='verified-bytecode-test-only-host-callable',
                    native_process='rust-stdlib-process-no-regex-abi', native_abi='not-implemented',
                    native_aot='not-claimed', status='adapter-ready' if stage == 'conf-ready' else
                    'verified-hosted-vm-adapter-and-native-stdlib-process')
            value['promotion']['next_blocks'] = [leaves[next_index]]
            value['implementation']['required_follow_ups'] = leaves[next_index:]
            assert accepts(value), stage
            states[stage] = value

        rejections = 0
        for stage, value in states.items():
            for section, key, replacement in [('implementation', 'public_api_promoted', True),
                    ('implementation', 'production_vm_registration', 'verified'),
                    ('implementation', 'native_abi', 'verified'),
                    ('implementation', 'native_aot_lowering', 'verified'),
                    ('implementation', 'required_follow_ups', []), ('promotion', 'next_blocks', [])]:
                invalid = copy.deepcopy(value)
                invalid[section][key] = replacement
                assert not accepts(invalid), (stage, section, key)
                rejections += 1
            if not stage.startswith('conf-'):
                invalid = copy.deepcopy(value)
                invalid['conformance'] = states['conf-ready']['conformance']
                assert not accepts(invalid) or stage == 'perf-verified', stage
                if stage != 'perf-verified':
                    rejections += 1
            else:
                for key, replacement in [('status', 'open'), ('native_abi', 'verified'),
                        ('native_aot', 'verified'), ('hosted_vm', 'public-api'),
                        ('native_process', 'native-aot'), ('extra', True)]:
                    invalid = copy.deepcopy(value)
                    invalid['conformance'][key] = replacement
                    assert not accepts(invalid), (stage, key)
                    rejections += 1
        self.assertEqual(len(states), 7)
        self.assertEqual(rejections, 58)

if __name__ == "__main__":
    unittest.main()
