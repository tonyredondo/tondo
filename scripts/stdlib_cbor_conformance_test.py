#!/usr/bin/env python3
"""Report-law negatives independent of Cargo/probe execution."""

import copy
import json
import pathlib
import tempfile
import unittest

import stdlib_cbor_conformance as c


class ConformanceLaws(unittest.TestCase):
    def setUp(self):
        self.path = c.ROOT / c.CONTRACT
        self.raw = self.path.read_bytes()
        self.contract = c.load_contract(self.path)
        self.vm = "\n".join(c.LINES.values()) + "\ncbor-conformance-ok\n"
        native = [{"id": key, "status": "passed", "line": line, "cleanup": True,
                   "live_runtime_objects": 0} for key, line in c.LINES.items()]
        native += [{"id": "cbor-conformance", "status": "passed"}]
        self.native = "\n".join(json.dumps(x) for x in native) + "\n"

    def report(self):
        report = {"format": "tondo-stdlib-cbor-conformance-evidence/1", "task": "STD-CBOR-CONF-001",
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
        for text in [self.vm.replace("InvalidUtf8:5:7", "InvalidUtf8:5:8"),
                     self.vm.replace(":270:", ":269:"), self.vm + "extra\n",
                     self.vm.replace("cbor-conformance-ok\n", "")]:
            with self.assertRaises(ValueError):
                c.compare(text, self.native)

    def test_contract_mutations_and_both_states(self):
        with tempfile.TemporaryDirectory(prefix="tondo-cbor-conf-law-") as directory:
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


if __name__ == "__main__":
    unittest.main()
