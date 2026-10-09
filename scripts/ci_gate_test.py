"""Orchestration/cache contract tests; fixture logs are not execution evidence."""

import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import ci_build_cache as cache
import test_gate_partitions as gate


class PartitionTests(unittest.TestCase):
    def test_every_canonical_command_has_exactly_one_owner_and_dependencies_are_final(self):
        plan = gate.plans()
        steps = [step for names in plan.values() for step in names]
        self.assertEqual(len(steps), len(set(steps)))
        self.assertIn("test", plan["foundation"])
        self.assertIn("layer-evidence-before", plan["foundation"])
        self.assertLess(plan["foundation"].index("conformance-standard-pin"), plan["foundation"].index("test"))
        self.assertIn("stdlib-log-performance", plan["stdlib"])
        self.assertIn("stdlib-channel-implementation", plan["runtime"])
        self.assertIn("native-source-scalars-tests", plan["native"])
        self.assertIn("stdlib-conformance", plan["final"])
        self.assertIn("stdlib-s1a-readiness-and-seal", plan["final"])
        self.assertEqual(plan, gate.plans())

    def test_duplicate_or_missing_dependency_boundary_is_refused(self):
        source = (gate.ROOT / "scripts/test-gate.sh").read_text()
        with self.assertRaises(ValueError): gate.plans(source + "\nrun_step test true\n")
        with self.assertRaises(ValueError): gate.plans(source.replace("run_step stdlib-public-api-audit ", "run_step replaced "))

    def workers(self, directory):
        source = {"fixture": "one exact source"}
        receipts = {}
        for name in gate.WORKERS:
            root = directory / name
            (root / "gate-partitions").mkdir(parents=True)
            (root / "logs").mkdir()
            (root / "metadata.json").write_text('{"fixture":true}\n')
            entries = []
            for step in gate.plans()[name]:
                file = root / "logs" / f"{step}.log"
                file.write_text(f"fixture only: {step}\n")
                entries.append({"name": step, "log_sha256": gate.digest(file)})
            value = {"format": gate.FORMAT, "partition": name, "status": "passed", "source": source, "steps": entries}
            file = root / "gate-partitions" / f"{name}.json"
            file.write_text(json.dumps(value))
            receipts[name] = file
        return source, receipts

    def test_complete_workers_merge_identical_metadata_and_verify_every_log(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, _ = self.workers(root / "inputs")
            received = gate.merge(root / "inputs", root / "output", source)
            self.assertEqual(set(received), set(gate.WORKERS))
            for name, value in received.items():
                gate.validate_receipt(value, name, root / "output", source)

    def test_missing_failed_drifted_duplicate_and_truncated_workers_are_refused(self):
        for mutation in ("missing", "failed", "source", "duplicate", "steps", "log"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source, paths = self.workers(root / "inputs")
                file = paths["foundation"]
                value = json.loads(file.read_text())
                if mutation == "missing": file.unlink()
                elif mutation == "log":
                    (file.parent.parent / "logs" / f"{value['steps'][0]['name']}.log").write_text("changed")
                elif mutation == "duplicate":
                    duplicate = root / "inputs/duplicate/gate-partitions"
                    duplicate.mkdir(parents=True)
                    (duplicate / "foundation.json").write_text(file.read_text())
                else:
                    if mutation == "failed": value["status"] = "failed"
                    elif mutation == "source": value["source"] = {"fixture": "different"}
                    else: value["steps"] = value["steps"][:-1]
                    file.write_text(json.dumps(value))
                with self.assertRaises((ValueError, OSError)):
                    gate.merge(root / "inputs", root / "output", source)
                self.assertFalse((root / "output").exists())

    def test_conflicting_metadata_or_existing_output_is_preserved_and_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, paths = self.workers(root / "inputs")
            (paths["native"].parent.parent / "metadata.json").write_text("different")
            with self.assertRaises(ValueError): gate.merge(root / "inputs", root / "output", source)
            self.assertFalse((root / "output").exists())
            (paths["native"].parent.parent / "metadata.json").write_text('{"fixture":true}\n')
            (root / "output").mkdir()
            (root / "output/metadata.json").write_text("preserve")
            with self.assertRaises(ValueError): gate.merge(root / "inputs", root / "output", source)
            self.assertEqual(list((root / "output").iterdir()), [root / "output/metadata.json"])
            self.assertEqual((root / "output/metadata.json").read_text(), "preserve")

    def test_fast_closure_requires_one_current_completed_plan(self):
        for mutation in ("none", "duplicate", "revision", "full", "boolean", "plan"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                summary = root / "summary.json"
                value = {"format": "tondo-fast-gate-evidence/1", "head": "current",
                         "scope": "impacted", "full_required": 0}
                (root / "plan.txt").write_text("test-tondo-cli cargo test -p tondo-cli\n")
                if mutation == "revision": value["head"] = "old"
                elif mutation == "full": value["full_required"] = 1
                elif mutation == "boolean": value["full_required"] = False
                elif mutation == "plan": (root / "plan.txt").write_text("")
                summary.write_text(json.dumps(value))
                if mutation == "duplicate":
                    (root / "duplicate").mkdir()
                    (root / "duplicate/summary.json").write_text(summary.read_text())
                if mutation == "none":
                    self.assertEqual(gate.verify_fast(root, "current"), "impacted")
                else:
                    with self.assertRaises(ValueError): gate.verify_fast(root, "current")
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError): gate.verify_fast(Path(directory), "current")


class BuildCacheTests(unittest.TestCase):
    def test_selection_keeps_only_external_units_and_respects_byte_budget(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for package, unit in (("serde", "a" * 16), ("tondo-compiler", "b" * 16)):
                folder = root / "debug/.fingerprint" / f"{package}-{unit}"
                folder.mkdir(parents=True)
                (folder / "lib.json").write_text("unit")
                deps = root / "debug/deps"
                deps.mkdir(exist_ok=True)
                (deps / f"lib{package.replace('-', '_')}-{unit}.rlib").write_bytes(b"x" * 64)
            (root / "debug/incremental").mkdir()
            (root / "debug/incremental/large").write_bytes(b"x" * 512)
            selected, size = cache.select(root, {"serde"}, 100)
            self.assertEqual(size, 68)
            self.assertEqual(len(selected), 2)
            self.assertFalse(any("tondo" in str(file) or "incremental" in str(file) for file in selected))
            self.assertEqual(cache.select(root, {"serde"}, 32), ([], 0))

    def fixture(self, root):
        relative = "debug/deps/libserde-" + "a" * 16 + ".rlib"
        file = root / "payload" / relative
        file.parent.mkdir(parents=True)
        file.write_bytes(b"fixture")
        value = {"format": cache.FORMAT, "signature": {"fixture": "same"}, "bytes": 7,
                 "entries": [{"path": relative, "size": 7, "sha256": cache.digest(file)}]}
        (root / "manifest.json").write_text(json.dumps(value))
        return value, file

    def test_cache_refuses_stale_identity_corruption_size_aliases_and_paths(self):
        for mutation in ("identity", "corrupt", "bytes", "boolean", "duplicate", "traversal", "absolute", "report", "incremental"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                value, file = self.fixture(root)
                cache.validated(root, {"fixture": "same"})
                if mutation == "identity": value["signature"] = {"fixture": "different"}
                elif mutation == "corrupt": file.write_bytes(b"edited!")
                elif mutation == "bytes": value["bytes"] += 1
                elif mutation == "boolean": value["entries"][0]["size"] = True
                elif mutation == "duplicate": value["entries"].append(copy.deepcopy(value["entries"][0]))
                else:
                    value["entries"][0]["path"] = {
                        "traversal": "debug/deps/../../outside", "absolute": "/outside",
                        "report": "reliability/evidence/test.json", "incremental": "debug/incremental/unit",
                    }[mutation]
                (root / "manifest.json").write_text(json.dumps(value))
                with self.assertRaises((ValueError, OSError)):
                    cache.validated(root, {"fixture": "same"})

    def test_restore_preserves_files_and_preflights_all_destinations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            value, _ = self.fixture(root / "cache")
            with patch.object(cache, "signature", return_value={"fixture": "same"}):
                cache.restore(root / "target", root / "cache")
                restored = root / "target" / value["entries"][0]["path"]
                self.assertEqual(restored.read_bytes(), b"fixture")
                with self.assertRaises(ValueError): cache.restore(root / "target", root / "cache")
                self.assertEqual(restored.read_bytes(), b"fixture")
                # A late collision must prevent even an earlier missing file from being copied.
                other = copy.deepcopy(value["entries"][0])
                other["path"] = other["path"].replace("serde", "other")
                file = root / "cache/payload" / other["path"]
                file.write_bytes(b"fixture")
                value["entries"].insert(0, other)
                value["bytes"] = 14
                (root / "cache/manifest.json").write_text(json.dumps(value))
                with self.assertRaises(ValueError): cache.restore(root / "target", root / "cache")
                self.assertFalse((root / "target" / other["path"]).exists())


if __name__ == "__main__":
    unittest.main()
