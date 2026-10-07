"""Report regressions for retained network observations and honest boundaries."""
from copy import deepcopy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import stdlib_net_performance as perf
from stdlib_net_performance import COUNTERS, TERMINAL, ROUTES, measurement, validate_sample, validate_measurement


def context_fixture():
    return {
        "suite": "tondo-stdlib-net-performance", "probe_sha256": "a" * 64,
        "source_tree_sha256": "b" * 64, "git_revision": "c" * 40,
        "target": "x86_64-unknown-linux-gnu", "backend": "rust-hosted-bridge", "profile": "test",
        "toolchain": {"rustc": "rustc 1.93.0 (fixture)", "cargo": "cargo 1.93.0 (fixture)"},
        "flags": {"rustflags": "", "encoded_rustflags": "", "incremental": "0"},
    }


def sample_fixture(name="tcp-read-small"):
    return [{
        "workload_id": name, "operation": ROUTES[name][0], "dispatch": "hosted-scalar",
        "process": process, "repetition": repetition, "nanos": 1000 + process * 100 + repetition,
        "observations": {
            **dict.fromkeys(COUNTERS, 0), "host_calls": 6, "polls": 3 + repetition,
            "partial_replies": repetition, "pending_polls": repetition,
            "selected_allocations": 12 + repetition, "bytes_copied": ROUTES[name][1],
            "sampled_budget_peak_bytes": 4096 + repetition, "reply_retained_peak_bytes": 128,
            "host_handles_created": 4 + repetition, "selected_fixture_bytes": 128,
            "selected_fixture_allocations": 2, "rejections": ROUTES[name][2],
        },
        "terminal": dict.fromkeys(TERMINAL, 0),
    } for process in range(1, 4) for repetition in range(9)]


class NetworkReportPreparation(unittest.TestCase):
    def capture(self, rows=None, context=None):
        return measurement(context or context_fixture(), "tcp-read-small",
                           sample_fixture() if rows is None else rows)

    def test_variable_counter_samples_are_retained_without_mutation(self):
        rows = sample_fixture()
        before = deepcopy(rows)
        result = self.capture(list(reversed(rows)))
        self.assertEqual(result["samples"], before)
        self.assertEqual(rows, before)
        self.assertEqual(result["observations"]["partial_replies"]["max"], 8)
        self.assertEqual(result["selected_logical_storage_estimate_bytes"]["min"], 4224)
        self.assertEqual(result["selected_logical_allocations"]["min"], 14)

    def test_each_route_retains_all_coordinates_and_honest_useful_bytes(self):
        for name, (operation, useful_bytes, _) in ROUTES.items():
            rows = sample_fixture(name)
            for row in rows:
                counters = row["observations"]
                if operation in {"tcp-write", "udp-send", "udp-receive"}:
                    counters["bytes_copied"] = 2 * useful_bytes
                if name in {"tls12-echo", "tls13-echo"}:
                    counters["bytes_copied"] = 96
                counters["dns_queries"] = 2 if operation == "dns" else 0
                counters["rollbacks"] = int(name == "tcp-read-rollback")
                counters["cancellations"] = int(name == "pending-accept-cancel")
                if name == "tcp-write-backpressure":
                    counters.update(partial_replies=1, pending_polls=16,
                                    backpressure_pending_polls=16, polls=17)
            result = measurement(context_fixture(), name, rows)
            self.assertEqual(len(result["samples"]), 27)
            self.assertEqual(result["throughput_payload_bytes_per_second"],
                             useful_bytes * 1_000_000_000 // result["latency_ns"]["median"])

    def test_outlier_p99_max_and_nearest_rank_p95(self):
        rows = sample_fixture()
        rows[-1]["nanos"] = 10_000_000_000
        result = self.capture(rows)
        self.assertEqual(result["latency_ns"]["p99"], 10_000_000_000)
        self.assertEqual(result["latency_ns"]["p95"], 1307)
        self.assertEqual(len(result["samples"]), 27)

    def test_missing_duplicate_bool_and_extra_process_are_refused(self):
        with self.assertRaises(ValueError):
            self.capture(sample_fixture()[:-1])
        rows = sample_fixture()
        rows[-1] = deepcopy(rows[0])
        with self.assertRaises(ValueError):
            self.capture(rows)
        for field, value in (("process", True), ("process", 4), ("repetition", -1)):
            rows = sample_fixture()
            rows[0][field] = value
            with self.assertRaises(ValueError):
                self.capture(rows)

    def test_admission_and_terminal_fields_are_exact(self):
        for key in TERMINAL:
            for value in (1, False, -1, None):
                rows = sample_fixture()
                rows[0]["terminal"][key] = value
                with self.assertRaises(ValueError):
                    self.capture(rows)
        for key, value in (("bytes_copied", 63), ("rejections", 1), ("dns_queries", 1),
                           ("pending_polls", 99), ("rollbacks", 1),
                           ("sampled_budget_peak_bytes", -1), ("selected_allocations", True)):
            rows = sample_fixture()
            rows[0]["observations"][key] = value
            with self.assertRaises(ValueError):
                self.capture(rows)

    def test_route_and_monotonic_latency_bounds(self):
        for key, value in (("nanos", 0), ("nanos", 30_000_000_001), ("nanos", True),
                           ("dispatch", "native"), ("workload_id", "unknown"), ("operation", "dns")):
            rows = sample_fixture()
            rows[0][key] = value
            with self.assertRaises(ValueError):
                self.capture(rows)

    def test_ambient_fields_are_rejected_and_workload_identity_is_bound(self):
        for key in ("path", "pid", "timestamp", "resolver_port", "ambient_environment"):
            context = context_fixture()
            context[key] = "ambient"
            with self.assertRaises(ValueError):
                self.capture(context=context)
        initial = self.capture()
        for key in ("probe_sha256", "source_tree_sha256", "git_revision"):
            context = context_fixture()
            context[key] = "d" * len(context[key])
            self.assertNotEqual(initial["identity_sha256"], self.capture(context=context)["identity_sha256"])

    def test_target_profile_toolchain_flags_and_identity_types(self):
        for key, value in (("target", "aarch64-unknown-linux-gnu"), ("backend", "native-aot"),
                           ("profile", "release"), ("probe_sha256", None), ("git_revision", "c" * 39)):
            context = context_fixture()
            context[key] = value
            with self.assertRaises(ValueError):
                self.capture(context=context)
        for key, value in (("toolchain", {"rustc": "rustc 1.94.0 fixture", "cargo": "cargo 1.93.0 fixture"}),
                           ("toolchain", {"rustc": None, "cargo": "cargo 1.93.0 fixture"}),
                           ("flags", {"incremental": False})):
            context = context_fixture()
            context[key] = value
            with self.assertRaises(ValueError):
                self.capture(context=context)

    def test_native_zero_and_forged_percentiles_or_metrics_are_rejected(self):
        original = self.capture()
        validate_measurement(context_fixture(), original)
        for key, value in (("native_live_handles", 0), ("throughput_operations_per_second", 1),
                           ("identity_sha256", "e" * 64), ("latency_ns", {"p99": 1})):
            result = deepcopy(original)
            result[key] = value
            with self.assertRaises(ValueError):
                validate_measurement(context_fixture(), result)

    def test_derived_integer_metrics_reject_decimal_and_boolean_substitutions(self):
        original = self.capture()
        for key in ("latency_ns", "observations"):
            result = deepcopy(original)
            values = result[key] if key == "latency_ns" else result[key]["bytes_copied"]
            values["median"] = float(values["median"])
            with self.assertRaises(ValueError):
                validate_measurement(context_fixture(), result)
        rows = sample_fixture()
        for row in rows:
            row["nanos"] = 1
        result = self.capture(rows)
        result["latency_ns"]["median"] = True
        with self.assertRaises(ValueError):
            validate_measurement(context_fixture(), result)

    def test_backpressure_requires_partial_progress_and_retained_pending(self):
        name = "tcp-write-backpressure"
        row = sample_fixture(name)[0]
        row["observations"]["bytes_copied"] *= 2
        with self.assertRaises(ValueError):
            validate_sample(name, row)
        row["observations"].update(partial_replies=1, pending_polls=16, polls=17)
        for count in (1, 15):
            row["observations"]["backpressure_pending_polls"] = count
            with self.assertRaises(ValueError):
                validate_sample(name, row)
        row["observations"]["backpressure_pending_polls"] = 16
        validate_sample(name, row)


def all_rows():
    rows = []
    for name, (operation, useful_bytes, _) in ROUTES.items():
        selected = sample_fixture(name)
        for row in selected:
            counters = row["observations"]
            if operation in {"tcp-write", "udp-send", "udp-receive"}:
                counters["bytes_copied"] = 2 * useful_bytes
            if name in {"tls12-echo", "tls13-echo"}:
                counters["bytes_copied"] = 96
            counters["dns_queries"] = 2 if operation == "dns" else 0
            counters["rollbacks"] = int(name == "tcp-read-rollback")
            counters["cancellations"] = int(name == "pending-accept-cancel")
            if name == "tcp-write-backpressure":
                counters.update(partial_replies=1, pending_polls=16,
                                backpressure_pending_polls=16, polls=17)
        rows.extend(selected)
    return rows


class CompleteNetworkReport(unittest.TestCase):
    def setUp(self):
        self.contract = perf.load_contract(Path("testing/stdlib-net-performance.json"))
        self.context = context_fixture()
        self.context["probe_sha256"] = self.contract["probe"]["sha256"]

    def report(self):
        return perf.render(self.contract, all_rows(), self.context,
                           {"cpu_model": "fixture", "os": "fixture"}, "development")

    def test_all_routes_and_all_567_samples_are_retained(self):
        report = self.report()
        self.assertEqual(len(report["measurements"]), 21)
        self.assertEqual(sum(len(item["samples"]) for item in report["measurements"]), 567)
        perf.validate_report(self.contract, report, self.context["source_tree_sha256"])

    def test_missing_duplicate_and_foreign_workloads_are_refused(self):
        for rows in (all_rows()[:-1], all_rows()[:-1] + [all_rows()[0]]):
            with self.assertRaises(ValueError):
                perf.measurements(self.context, rows)
        rows = all_rows()
        rows[0]["workload_id"] = "foreign"
        with self.assertRaises(ValueError):
            perf.measurements(self.context, rows)

    def test_capture_and_current_source_are_required(self):
        report = self.report()
        with self.assertRaises(ValueError):
            perf.validate_report(self.contract, report, "d" * 64)
        report["capture"] = "published"
        with self.assertRaises(ValueError):
            perf.validate_report(self.contract, report, self.context["source_tree_sha256"])

    def test_clean_capture_rejects_missing_or_different_committed_probe(self):
        report = self.report()
        report["capture"] = "clean-source"
        for returncode, output in ((1, b""), (0, b"different probe")):
            result = perf.subprocess.CompletedProcess([], returncode, stdout=output)
            with patch.object(perf.subprocess, "run", return_value=result):
                with self.assertRaises(ValueError):
                    perf.validate_report(self.contract, report, self.context["source_tree_sha256"])

    def test_clean_capture_checks_actual_committed_probe_bytes(self):
        report = self.report()
        report["capture"] = "clean-source"
        result = perf.subprocess.CompletedProcess([], 0, stdout=(perf.ROOT / perf.PROBE).read_bytes())
        with patch.object(perf.subprocess, "run", return_value=result) as run:
            perf.validate_report(self.contract, report, self.context["source_tree_sha256"])
        self.assertEqual(run.call_args.args[0][-1], f"{self.context['git_revision']}:{perf.PROBE}")

    def test_strategy_resources_and_native_claims_cannot_drift(self):
        for key, value in (("strategy", {"native_aot": "verified"}),
                           ("resource_model", {"logical_memory": "RSS"}),
                           ("protocol", {"outliers": "delete"}), ("target", "tondo-vm-hosted")):
            report = self.report()
            report[key] = value
            with self.assertRaises(ValueError):
                perf.validate_report(self.contract, report, self.context["source_tree_sha256"])

    def test_protocol_numbers_reject_equal_decimal_values(self):
        # A captured report is read independently of the contract; modifying an
        # in-memory render's shared dictionary would alter both test inputs.
        report = deepcopy(self.report())
        report["protocol"]["independent_processes"] = 3.0
        with self.assertRaises(ValueError):
            perf.validate_report(self.contract, report, self.context["source_tree_sha256"])

    def test_pinned_contract_numbers_and_boolean_descriptor_require_exact_types(self):
        with tempfile.TemporaryDirectory(prefix="tondo-net-performance-types-") as temporary:
            path = Path(temporary) / "contract.json"
            for section, key, value in (("protocol", "independent_processes", 3.0),
                                        ("limits", "provider_wait_seconds", 5.0),
                                        ("report", "portable_artifact", 0)):
                contract = deepcopy(self.contract)
                contract[section][key] = value
                path.write_text(json.dumps(contract, indent=2) + "\n")
                with self.assertRaises(ValueError):
                    perf.load_contract(path)
            contract = deepcopy(self.contract)
            contract["workloads"][0]["payload_bytes"] = False
            path.write_text(json.dumps(contract, indent=2) + "\n")
            with self.assertRaises(ValueError):
                perf.load_contract(path)

    def test_host_description_does_not_enter_stable_identity(self):
        report = self.report()
        identities = [item["identity_sha256"] for item in report["measurements"]]
        report["host"] = {"cpu_model": "different host", "os": "different OS"}
        perf.validate_report(self.contract, report, self.context["source_tree_sha256"])
        self.assertEqual(identities, [item["identity_sha256"] for item in report["measurements"]])
        report["host"]["pid"] = 1
        with self.assertRaises(ValueError):
            perf.validate_report(self.contract, report, self.context["source_tree_sha256"])


if __name__ == "__main__":
    unittest.main()
