"""Focused laws for retained measurements and truthful CBOR report boundaries."""

import copy
import hashlib
import json
import pathlib
import tempfile
import unittest
from unittest.mock import patch

import stdlib_cbor_performance as perf


class PerformanceReports(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.contract = perf.load_contract(perf.ROOT / "testing/stdlib-cbor-performance.json")
        cls.tree = "a" * 64
        cls.context = {
            "git_revision": "b" * 40, "source_tree_sha256": cls.tree,
            "toolchain": {"rustc": "rustc 1.93.0 (fixture)", "cargo": "cargo 1.93.0 (fixture)"},
            "flags": {"rustflags": "", "encoded_rustflags": "", "incremental": "0"},
            "host": {"cpu_model": "synthetic", "os": "synthetic"}, "capture": "development",
        }
        # These are synthetic validator inputs, never benchmark evidence. The
        # retained maximum is deliberately far from the rest of the samples.
        cls.rows = [{
            "workload_id": spec["id"], "operation": spec["operation"],
            "process": process, "repetition": repetition,
            "nanos": 1_000_000 if (process, repetition) == (3, 8) else 100 * ((process - 1) * 9 + repetition + 1),
            "dispatch": spec["dispatch"], "counters": copy.deepcopy(spec["counters"]),
        } for spec in cls.contract["workloads"] for process in range(1, 4) for repetition in range(9)]
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

    def test_distribution_keeps_outlier_and_process_coordinates(self):
        for item in self.report["measurements"]:
            self.assertEqual((item["median_ns"], item["p95_ns"], item["p99_ns"]), (1400, 2600, 1_000_000))
            self.assertEqual(item["samples"][-1], {"process": 3, "repetition": 8, "nanos": 1_000_000})
            self.assertEqual(item["throughput_operations_per_second"], 16_000_000_000 // 1400)
            transported = item["counters"]["output_bytes"] or item["counters"]["input_bytes"]
            self.assertEqual(item["throughput_bytes_per_second"], transported * 16_000_000_000 // 1400)
        self.assertEqual(perf.render(self.contract, list(reversed(self.rows)), self.context), self.report)

    def test_missing_duplicate_and_invalid_samples_are_rejected(self):
        mutations = {
            "missing": lambda rows: rows.pop(),
            "duplicate-coordinate": lambda rows: rows[-1].update(process=3, repetition=7),
            "process-zero": lambda rows: rows[0].update(process=0),
            "process-pid": lambda rows: rows[0].update(process=12345),
            "boolean-process": lambda rows: rows[0].update(process=True),
            "extra-repetition": lambda rows: rows[0].update(repetition=9),
            "negative-latency": lambda rows: rows[0].update(nanos=-1),
            "float-latency": lambda rows: rows[0].update(nanos=1.25),
            "boolean-latency": lambda rows: rows[0].update(nanos=True),
            "unknown-workload": lambda rows: rows[0].update(workload_id="unmeasured"),
            "native-route": lambda rows: rows[0].update(dispatch="native"),
            "wrong-operation": lambda rows: rows[0].update(operation="materialized-encode"),
            "ambient-field": lambda rows: rows[0].update(pid=12345),
        }
        for name, change in mutations.items():
            with self.subTest(name=name):
                self.reject_rows(change)

    def test_corrupted_metrics_provenance_and_scope_are_rejected(self):
        mutations = {
            "tail": lambda report: report["measurements"][0].update(p99_ns=2600),
            "throughput": lambda report: report["measurements"][0].update(throughput_bytes_per_second=0),
            "identity": lambda report: report["measurements"][0].update(identity_sha256="0" * 64),
            "source": lambda report: report.update(source_tree_sha256="0" * 64),
            "probe": lambda report: report.update(probe_sha256="0" * 64),
            "revision": lambda report: report.update(git_revision="main"),
            "sample-count": lambda report: report["measurements"][0].update(sample_count=26),
            "missing-sample": lambda report: report["measurements"][0]["samples"].pop(),
            "ambient-report": lambda report: report.update(timestamp="now"),
            "native-claim": lambda report: report["strategy"].update(native_aot="verified"),
            "hosted-claim": lambda report: report["strategy"].update(hosted_vm="verified"),
            "allocator-claim": lambda report: report["resource_model"].update(allocations="OS calls"),
            "wrong-toolchain": lambda report: report["toolchain"].update(rustc="rustc 1.94.0 (fixture)"),
            "malformed-toolchain": lambda report: report["toolchain"].update(cargo=123),
            "missing-flags": lambda report: report["flags"].pop("rustflags"),
            "invalid-capture": lambda report: report.update(capture="verified"),
        }
        for name, change in mutations.items():
            with self.subTest(name=name):
                self.reject_report(change)

    def test_logical_resources_and_rejections_cannot_publish_partial_success(self):
        for spec in self.contract["workloads"]:
            for key, value in {"native_live_handles": 0, "terminal_open_streams": 1,
                               "allocations": -1, "logical_memory_bytes": True, "nodes": 129}.items():
                counters = copy.deepcopy(spec["counters"])
                counters[key] = value
                with self.subTest(workload=spec["id"], field=key), self.assertRaises(ValueError):
                    perf.validate_counters(counters, spec["id"])
            if spec["id"] in perf.ERRORS:
                for key, value in {"output_bytes": 1, "bytes_copied": 1, "adversarial_rejections": 15}.items():
                    counters = copy.deepcopy(spec["counters"])
                    counters[key] = value
                    with self.subTest(workload=spec["id"], field=key), self.assertRaises(ValueError):
                        perf.validate_counters(counters, spec["id"])
        self.reject_rows(lambda rows: rows[0]["counters"].update(allocations=rows[0]["counters"]["allocations"] + 1))

    def test_identity_excludes_descriptive_host_and_binds_build_context(self):
        context = copy.deepcopy(self.context)
        context["host"] = {"cpu_model": "another descriptive CPU", "os": "another OS"}
        identities = [item["identity_sha256"] for item in self.report["measurements"]]
        self.assertEqual([item["identity_sha256"] for item in perf.render(self.contract, self.rows, context)["measurements"]], identities)
        for field, value in {"git_revision": "c" * 40, "source_tree_sha256": "d" * 64,
                             "flags": {"rustflags": "-C debuginfo=1", "encoded_rustflags": "", "incremental": "0"}}.items():
            context = copy.deepcopy(self.context)
            context[field] = value
            self.assertNotEqual([item["identity_sha256"] for item in perf.render(self.contract, self.rows, context)["measurements"]], identities)

    def test_clean_capture_requires_the_exact_committed_probe(self):
        context = copy.deepcopy(self.context)
        context["capture"] = "clean-source"
        probe = (perf.ROOT / perf.PROBE).read_bytes()
        self.assertEqual(hashlib.sha256(probe).hexdigest(), self.contract["probe"]["sha256"])
        with patch.object(perf.subprocess, "run", return_value=perf.subprocess.CompletedProcess([], 0, probe)) as call:
            report = perf.render(self.contract, self.rows, context)
            self.assertEqual(report["capture"], "clean-source")
            self.assertEqual(call.call_args.args[0][-1], f"{context['git_revision']}:{perf.PROBE}")
        for result in [perf.subprocess.CompletedProcess([], 1, b""),
                       perf.subprocess.CompletedProcess([], 0, probe + b"changed")]:
            with patch.object(perf.subprocess, "run", return_value=result), self.assertRaises(ValueError):
                perf.render(self.contract, self.rows, context)

    def test_contract_pins_resource_semantics_and_actual_probe_bytes(self):
        mutations = {
            "lost-samples": lambda value: value["protocol"].update(independent_processes=1),
            "native-promotion": lambda value: value["strategy"].update(native_runtime_abi="verified"),
            "unrecorded-profile": lambda value: value.update(profile="release"),
            "stale-probe": lambda value: value["probe"].update(sha256="0" * 64),
            "model-claims-RSS": lambda value: value["resource_model"].update(logical_memory="RSS"),
            "model-claims-allocator": lambda value: value["resource_model"].update(allocations="OS calls"),
            "hidden-model-exclusion": lambda value: value["resource_model"]["excluded"].pop(),
            "duplicate-metric": lambda value: value["metrics"].append(value["metrics"][0]),
            "missing-workload": lambda value: value["workloads"].pop(),
            "wrong-error": lambda value: value["workloads"][-1].update(expected_error=None),
        }
        with tempfile.TemporaryDirectory(prefix="tondo-cbor-report-test-") as folder:
            candidate = pathlib.Path(folder) / "contract.json"
            for name, change in mutations.items():
                value = copy.deepcopy(self.contract)
                change(value)
                candidate.write_text(json.dumps(value, indent=2) + "\n")
                with self.subTest(name=name), self.assertRaises((ValueError, KeyError, TypeError)):
                    perf.load_contract(candidate)
            for suffix in ["", "\n\n", "\r\n", " \n"]:
                candidate.write_text(json.dumps(self.contract) + suffix)
                with self.subTest(suffix=repr(suffix)), self.assertRaises(ValueError):
                    perf.load_contract(candidate)


if __name__ == "__main__":
    unittest.main()
