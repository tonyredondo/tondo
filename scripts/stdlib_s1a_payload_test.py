"""Bounded record tests; these fixtures are not S1A promotion evidence."""

import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from stdlib_s1a_payload import digest, verify_payload


class PayloadChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        self.bundle = self.base / "bundle"
        self.manifest = {"revision": "1" * 40, "source_tree_sha256": "2" * 64,
                         "payload_sha256": "3" * 64, "files": []}
        self.seal = {**self.manifest, "task": "STD-S1A-SEAL-001", "edition": "0.1", "phase": "STD-0.1A"}
        self.claims = {"g5": False, "native_backend": False, "tlf": False, "public_release": False}
        names = ["testing/stdlib-s1a-seal.json", "testing/stdlib-public-api.json",
                 "testing/stdlib-matrix.json", "testing/stdlib-owner-evidence.json",
                 "testing/stdlib-conformance.json", "testing/inventory.json",
                 "conformance/0.1/manifest.json", "conformance/draft/manifest.json"]
        self.contract = {
            "required_inputs": names, "required_checks": ["synthetic-record-check"],
            "required_evidence": [{"path": f"target/{name}.json", "format": name,
                                   "status": "passed", "revision_field": "revision",
                                   "tree_field": "tree_sha256"}
                                  for name in ("stdlib-conformance", "stdlib-distribution")],
            "invariants": {"claims": self.claims, "public_api": {"signatures": 1, "owners": 1},
                           "matrix": {"owners": 1, "requirements": 0, "rows": 1},
                           "fuzz": {"owners": 1}, "conformance": {"cases": 1}},
        }
        self.contract_path = self.base / "contract.json"
        raw = self.json_bytes(self.contract)
        self.contract_path.write_bytes(raw)
        self.put("inputs/testing/stdlib-s1a-seal.json", raw)
        self.put("inputs/testing/stdlib-public-api.json", {
            "status": "verified", "summary": {"gaps": 0, "signatures": 1, "verified": 1},
            "rows": [{"id": "synthetic", "owner": "std.fixture", "status": "verified", "missing": []}],
            "owners": [{"id": "std.fixture", "status": "verified"}]})
        self.put("inputs/testing/stdlib-matrix.json", {
            "status": "verified", "summary": {"owners": 1, "requirements": 0, "rows": 1, "open_rows": 0},
            "rows": [{"id": "synthetic", "owner": "std.fixture", "status": "verified"}],
            "owners": [{"id": "std.fixture", "stages": {stage: {"status": "verified"}
                for stage in ("SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC")}}]})
        self.put("inputs/testing/stdlib-owner-evidence.json", {
            "status": "promoted-evidence", "owners": [{"id": "std.fixture", "cells": {"FUZZ": {"status": "verified", "reason": None}}}]})
        self.put("inputs/testing/stdlib-conformance.json", {"scope": "synthetic record test"})
        self.put("inputs/testing/inventory.json", {"tests": []})
        self.put("inputs/conformance/draft/manifest.json", {"lineage": "synthetic"})
        self.put("inputs/conformance/0.1/manifest.json", {"cases": [{"id": "synthetic"}]})
        suite_hash = self.hash("inputs/conformance/0.1/manifest.json")
        draft_hash = self.hash("inputs/conformance/draft/manifest.json")
        identity = {"tree_sha256": self.seal["source_tree_sha256"],
                    "inventory_sha256": self.hash("inputs/testing/inventory.json"),
                    "input_set_sha256": "4" * 64, "passed": True}
        self.put("evidence/layer-evidence.json", {**identity, "manifest_sha256": draft_hash})
        self.put("evidence/conformance-result.json", {**identity, "manifest_sha256": suite_hash,
                 "lineage_manifest_sha256": draft_hash, "cases": [{"id": "synthetic"}]})
        self.put("evidence/stdlib-conformance-logs/synthetic.log", b"synthetic observation\n")
        self.put("evidence/stdlib-conformance.json", {
            "format": "stdlib-conformance", "status": "passed", "revision": self.seal["revision"],
            **identity, "promotion": "promoted", "public_row_coverage": "verified",
            "contract_sha256": self.hash("inputs/testing/stdlib-conformance.json"),
            "manifest_sha256": suite_hash, "full_suite": {"cases": 1, "passed": True,
                "result_sha256": self.hash("evidence/conformance-result.json")},
            "commands": [{"status": "passed", "log": "synthetic.log",
                "log_sha256": self.hash("evidence/stdlib-conformance-logs/synthetic.log")}]})
        self.put("distribution/tondo-std-0.1.tar", b"synthetic nested archive identity")
        self.put("evidence/stdlib-distribution.json", {
            "format": "stdlib-distribution", "status": "passed", "revision": self.seal["revision"],
            **identity, "promotion": "promoted", "inputs": {"source_revision": self.seal["revision"],
                "source_dirty": False, "binary_source_provenance": "verified"},
            "byte_identical": True, "clean_source_workspaces": 2, "public_release": False,
            "archive_sha256": self.hash("distribution/tondo-std-0.1.tar")})
        self.put("commands/check-01.log", b"synthetic command transcript\n")
        commands = [{"id": "check-01", "command": "synthetic-record-check", "status": "passed",
                     "log": "commands/check-01.log", "log_sha256": self.hash("commands/check-01.log")}]
        self.put("metadata/commands.json", commands)
        self.put("metadata/verification.json", {
            "format": "tondo-stdlib-s1a-verification/1", **{k: self.seal[k] for k in
                ("task", "edition", "phase", "revision", "source_tree_sha256")},
            "contract_sha256": digest(raw), "claims": self.claims, "checks": commands})

    @staticmethod
    def json_bytes(value):
        return (json.dumps(value, sort_keys=True) + "\n").encode()

    def put(self, path, value):
        destination = self.bundle / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(value if isinstance(value, bytes) else self.json_bytes(value))
        if not any(entry["path"] == path for entry in self.manifest["files"]):
            self.manifest["files"].append({"path": path})

    def hash(self, path):
        return digest((self.bundle / path).read_bytes())

    def verify(self):
        verify_payload(self.bundle, self.contract_path, self.seal, self.manifest)

    def test_consistent_bounded_records_pass_without_execution(self):
        self.verify()

    def test_each_required_payload_cannot_be_omitted(self):
        original = copy.deepcopy(self.manifest)
        for entry in original["files"]:
            with self.subTest(path=entry["path"]):
                self.manifest["files"] = [item for item in original["files"] if item != entry]
                with self.assertRaises(ValueError):
                    self.verify()
        self.manifest = original
        self.verify()

    def test_manifest_identity_drift_is_rejected(self):
        for key in ("revision", "source_tree_sha256", "payload_sha256"):
            with self.subTest(field=key):
                old = self.manifest[key]
                self.manifest[key] = "0" * len(old)
                with self.assertRaisesRegex(ValueError, "differs from seal"):
                    self.verify()
                self.manifest[key] = old

    def test_empty_duplicate_or_unrelated_owner_records_are_rejected(self):
        for path in ("inputs/testing/stdlib-public-api.json", "inputs/testing/stdlib-matrix.json",
                     "inputs/testing/stdlib-owner-evidence.json"):
            original = json.loads((self.bundle / path).read_bytes())
            replacements = [[], original["owners"] * 2,
                            [{**original["owners"][0], "id": "std.other"}]]
            if path.endswith("stdlib-matrix.json"):
                replacements.append([{**original["owners"][0], "stages": {}}])
            for owners in replacements:
                with self.subTest(path=path, owners=owners):
                    self.put(path, {**original, "owners": owners})
                    with self.assertRaises(ValueError):
                        self.verify()
            self.put(path, original)
        self.verify()

    def test_changed_transcripts_and_copied_contract_are_rejected(self):
        for path in ("commands/check-01.log", "evidence/stdlib-conformance-logs/synthetic.log",
                     "distribution/tondo-std-0.1.tar", "inputs/testing/stdlib-s1a-seal.json"):
            with self.subTest(path=path):
                original = (self.bundle / path).read_bytes()
                self.put(path, original + b"changed")
                with self.assertRaises(ValueError):
                    self.verify()
                self.put(path, original)

    def test_agreeing_command_records_cannot_substitute_required_checks(self):
        commands_path = "metadata/commands.json"
        verification_path = "metadata/verification.json"
        original_commands = json.loads((self.bundle / commands_path).read_bytes())
        original_verification = json.loads((self.bundle / verification_path).read_bytes())
        substituted = copy.deepcopy(original_commands)
        substituted[0]["command"] = "different-check"
        for commands in ([], substituted, original_commands * 2):
            with self.subTest(commands=commands):
                verification = copy.deepcopy(original_verification)
                verification["checks"] = commands
                self.put(commands_path, commands)
                self.put(verification_path, verification)
                with self.assertRaisesRegex(ValueError, "required checks"):
                    self.verify()
        self.put(commands_path, original_commands)
        self.put(verification_path, original_verification)
        self.verify()

    def test_rehashed_case_results_still_require_complete_successful_suite(self):
        result_path = "evidence/conformance-result.json"
        report_path = "evidence/stdlib-conformance.json"
        original_result = json.loads((self.bundle / result_path).read_bytes())
        original_report = json.loads((self.bundle / report_path).read_bytes())
        for update in ({"passed": False}, {"cases": []}, {"cases": [{"id": "other"}]},
                       {"tree_sha256": "0" * 64}, {"inventory_sha256": "0" * 64},
                       {"lineage_manifest_sha256": "0" * 64}):
            with self.subTest(update=update):
                self.put(result_path, {**original_result, **update})
                report = copy.deepcopy(original_report)
                report["full_suite"]["result_sha256"] = self.hash(result_path)
                self.put(report_path, report)
                with self.assertRaisesRegex(ValueError, "results or layer evidence"):
                    self.verify()
        self.put(result_path, original_result)
        self.put(report_path, original_report)
        self.verify()

    def test_open_states_and_substituted_evidence_are_rejected(self):
        changes = [
            ("metadata/verification.json", lambda r: r.update(revision="0" * 40)),
            ("metadata/verification.json", lambda r: r.update(checks=[])),
            ("metadata/commands.json", lambda r: r[0].update(command="substituted")),
            ("inputs/testing/stdlib-public-api.json", lambda r: r["summary"].update(gaps=1)),
            ("inputs/testing/stdlib-matrix.json", lambda r: r["summary"].update(open_rows=1)),
            ("inputs/testing/stdlib-owner-evidence.json", lambda r: r["owners"][0]["cells"]["FUZZ"].update(status="partial")),
            ("evidence/stdlib-conformance.json", lambda r: r.update(promotion="pending")),
            ("evidence/stdlib-conformance.json", lambda r: r.update(public_row_coverage="unverified")),
            ("evidence/stdlib-conformance.json", lambda r: r.update(revision="0" * 40)),
            ("evidence/conformance-result.json", lambda r: r.update(passed=False)),
            ("evidence/layer-evidence.json", lambda r: r.update(passed=False)),
            ("evidence/stdlib-distribution.json", lambda r: r.update(promotion="pending")),
            ("evidence/stdlib-distribution.json", lambda r: r["inputs"].update(source_dirty=True)),
            ("evidence/stdlib-distribution.json", lambda r: r["inputs"].update(binary_source_provenance="not-claimed")),
        ]
        for path, change in changes:
            with self.subTest(path=path):
                original = (self.bundle / path).read_bytes()
                altered = json.loads(original)
                change(altered)
                self.put(path, altered)
                with self.assertRaises(ValueError):
                    self.verify()
                self.put(path, original)
        self.verify()

    def test_integrity_only_archive_does_not_pass_the_public_checker(self):
        root = self.base / "integrity-only"
        bundle = root / "bundle/tondo-stdlib-s1a"
        (bundle / "metadata").mkdir(parents=True)
        payload = b"controlled integrity-only fixture\n"
        (bundle / "proof.txt").write_bytes(payload)
        payload_hash = digest(f"proof.txt\t{digest(payload)}\t{len(payload)}\n".encode())
        manifest = {"format": "tondo-stdlib-s1a-bundle/1", "task": self.seal["task"],
                    "edition": "0.1", "phase": "STD-0.1A", "root": "tondo-stdlib-s1a",
                    "claims": self.claims, "files": [{"path": "proof.txt", "sha256": digest(payload),
                                                      "bytes": len(payload), "origin": "fixture"}]}
        raw = self.json_bytes(manifest)
        (bundle / "metadata/manifest.json").write_bytes(raw)
        archive = root / f"tondo-stdlib-s1a-{payload_hash}.tar"
        subprocess.run(["tar", "--format=ustar", "--sort=name", "-cf", str(archive),
                        "-C", str(root / "bundle"), "tondo-stdlib-s1a"], check=True)
        seal = {**self.seal, "format": "tondo-stdlib-s1a-seal/1", "status": "sealed-draft",
                "manifest": "bundle/tondo-stdlib-s1a/metadata/manifest.json",
                "bundle_id": f"tondo-stdlib-s1a-{payload_hash}", "payload_sha256": payload_hash,
                "manifest_sha256": digest(raw), "archive": archive.name,
                "archive_sha256": digest(archive.read_bytes()), "public_release": False,
                "g5_claim": False, "native_backend_claim": False, "tlf_claim": False,
                "next_block": "DIAG-SPEC-001"}
        (root / "seal.json").write_bytes(self.json_bytes(seal))
        import os
        environment = dict(os.environ, TONDO_STDLIB_S1A_SEAL_DIR=str(root))
        environment.pop("TONDO_STDLIB_S1A_SEAL_CONTRACT", None)
        check = subprocess.run([str(Path(__file__).with_name("stdlib-s1a-seal-check.sh"))],
                               env=environment, capture_output=True, text=True)
        self.assertNotEqual(check.returncode, 0)
        self.assertIn("bundle omits required inputs or evidence", check.stderr)


if __name__ == "__main__":
    unittest.main()
