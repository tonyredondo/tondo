"""Contract algorithm tests; fabricated timings are never benchmark evidence."""

import copy
from pathlib import Path
import unittest

import stdlib_log_performance as report

ROUTES = {"filtered": {"operation": "filter", "expected_counters": {
    "operations": 128, "attempted_events": 128, "filtered_events": 128,
    "accepted_events": 0, "dropped_events": 0, "rejected_events": 0, "output_bytes": 0,
    "delivered_records": 0,
    "model_peak_queued_records": 0, "model_peak_queued_bytes": 0,
}}}


def rows():
    result = []
    for process in range(1, 4):
        for repetition in range(9):
            counters = {field: 1 for field in report.COUNTERS}
            counters.update(ROUTES["filtered"]["expected_counters"])
            terminal = {field: 0 for field in report.TERMINAL_FIELDS}
            terminal["budget_bytes"] = None
            result.append({"workload_id": "filtered", "operation": "filter", "process": process,
                           "repetition": repetition, "nanos": len(result) + 1,
                           "dispatch": "hosted-scalar", "counters": counters,
                           "terminal": terminal,
                           "native_live_handles": None})
    return result


class ReportTests(unittest.TestCase):
    def test_nearest_rank_retains_outlier_and_every_coordinate(self):
        samples = rows()
        samples[-1]["nanos"] = 30_000_000_000
        value = report.render_workloads(samples, ROUTES)
        self.assertEqual(value[0]["latency_ns"], {"median": 14, "p95": 26,
                                                 "p99": 30_000_000_000, "max": 30_000_000_000})
        self.assertEqual(len(value[0]["samples"]), 27)
        self.assertEqual(value[0]["accepted_events_per_second"], 0)
        self.assertEqual(value[0]["delivered_records_per_second"], 0)
        report.verify_workloads(value, ROUTES)

    def test_input_order_does_not_change_report(self):
        self.assertEqual(report.render_workloads(rows(), ROUTES),
                         report.render_workloads(list(reversed(rows())), ROUTES))

    def test_missing_duplicate_unknown_and_invalid_sample_are_refused(self):
        mutations = []
        mutations.append(rows()[:-1])
        value = rows(); value[-1] = copy.deepcopy(value[0]); mutations.append(value)
        for field, replacement in (("process", 0), ("process", True), ("repetition", 9),
                                   ("nanos", 0), ("nanos", 30_000_000_001), ("nanos", 1.5),
                                   ("workload_id", "unknown"), ("dispatch", "native-aot"),
                                   ("native_live_handles", 0)):
            value = rows(); value[0][field] = replacement; mutations.append(value)
        value = rows(); value[0]["path"] = "/ambient"; mutations.append(value)
        for value in mutations:
            with self.subTest(value=value[0]):
                with self.assertRaises(ValueError): report.render_workloads(value, ROUTES)

    def test_counter_type_bounds_oracle_and_terminal_refusals(self):
        for field, replacement in (("vm_steps", False), ("vm_allocations", 0),
                                   ("vm_peak_live_bytes", 2_147_483_649), ("filtered_events", 127)):
            value = rows(); value[0]["counters"][field] = replacement
            with self.subTest(field=field):
                with self.assertRaises(ValueError): report.render_workloads(value, ROUTES)
        for terminal in ({"host_handles": 1}, {"host_handles": False}, {"unexpected": 0}):
            value = rows(); value[0]["terminal"].update(terminal)
            with self.subTest(terminal=terminal):
                with self.assertRaises(ValueError): report.render_workloads(value, ROUTES)
        value = rows(); value[0]["terminal"]["budget_bytes"] = 0
        with self.assertRaises(ValueError): report.render_workloads(value, ROUTES)

    def test_edited_summary_does_not_replace_retained_samples(self):
        original = report.render_workloads(rows(), ROUTES)
        for field, replacement in (("operations_per_second", 0), ("accepted_events_per_second", 1),
                                   ("sample_count", 26), ("dispatch", "native-aot")):
            value = copy.deepcopy(original); value[0][field] = replacement
            with self.subTest(field=field):
                with self.assertRaises(ValueError): report.verify_workloads(value, ROUTES)
        value = copy.deepcopy(original); value[0]["counter_distributions"]["vm_steps"]["max"] = 999
        with self.assertRaises(ValueError): report.verify_workloads(value, ROUTES)

    def test_strict_json_refuses_duplicates_and_non_finite_numbers(self):
        for raw in ('{"x":1,"x":2}', '{"outer":{"x":1,"x":2}}', '{"x":NaN}',
                    '{"x":Infinity}', '{"x":-Infinity}', '{"x":1e309}'):
            with self.subTest(raw=raw):
                with self.assertRaises(ValueError): report.strict_json(raw)
        self.assertEqual(report.strict_json('{"x":1}'), {"x": 1})

    def test_summary_boolean_cannot_impersonate_integer_one(self):
        value = report.render_workloads(rows(), ROUTES)
        value[0]["counter_distributions"]["vm_steps"]["min"] = True
        with self.assertRaises(ValueError): report.verify_workloads(value, ROUTES)

    def test_stable_identity_refuses_ambient_fields_and_false_native_route(self):
        value = {"suite": "stdlib-log", "workload_id": "filtered", "probe_sha256": "a" * 64,
                 "source_tree_sha256": "b" * 64, "target": "x86_64-unknown-linux-gnu",
                 "backend": "hosted-bytecode-vm", "profile": "test",
                 "toolchain": {"rustc": "rustc 1.93.0", "cargo": "cargo 1.93.0"},
                 "flags": {"rustflags": "", "encoded_rustflags": "", "incremental": "0"},
                 "git_revision": "c" * 40}
        report.validate_identity(value)
        self.assertEqual(report.digest(value), report.digest(dict(reversed(list(value.items())))))
        for field, replacement in (("pid", 123), ("path", "/tmp/log"), ("timestamp", "now"),
                                   ("backend", "native-aot"), ("git_revision", "main"),
                                   ("probe_sha256", "A" * 64), ("flags", {})):
            mutated = copy.deepcopy(value); mutated[field] = replacement
            with self.subTest(field=field):
                with self.assertRaises(ValueError): report.validate_identity(mutated)


def complete_rows():
    result = []
    for name, route in report.ROUTES.items():
        for process in range(1, 4):
            for repetition in range(9):
                counters = {field: 1 for field in report.COUNTERS}
                counters.update(vm_steps=100, vm_peak_live_bytes=256, selected_fixture_bytes=128,
                                model_peak_queued_bytes=32 * route["expected_counters"]["model_peak_queued_records"],
                                output_bytes=32 * route["expected_counters"]["delivered_records"])
                counters.update(route["expected_counters"])
                terminal = {field: 0 for field in report.TERMINAL_FIELDS}
                terminal["budget_bytes"] = None
                result.append({"workload_id": name, "operation": route["operation"], "process": process,
                               "repetition": repetition, "nanos": process * 100 + repetition,
                               "dispatch": "hosted-scalar", "counters": counters, "terminal": terminal,
                               "native_live_handles": None})
    return result


def context():
    return {"suite": "stdlib-log", "probe_sha256": report.contract_value("measurement-ready")["probe"]["sha256"],
            "source_tree_sha256": "b" * 64, "target": "x86_64-unknown-linux-gnu",
            "backend": "hosted-bytecode-vm", "profile": "test",
            "toolchain": {"rustc": "rustc 1.93.0", "cargo": "cargo 1.93.0"},
            "flags": {"rustflags": "", "encoded_rustflags": "", "incremental": "0"},
            "git_revision": "c" * 40}


class CompleteProtocolTests(unittest.TestCase):
    def setUp(self):
        self.contract = report.contract_value("measurement-ready")
        self.report = report.render(self.contract, complete_rows(), context(),
                                    {"cpu_model": "test fixture", "os": "test fixture"}, "development")

    def test_every_authored_route_retains_all_samples_and_identity(self):
        self.assertEqual(len(self.report["workloads"]), 18)
        self.assertEqual(sum(len(item["samples"]) for item in self.report["workloads"]), 486)
        report.validate_report(self.contract, self.report, "b" * 64)
        self.assertEqual(self.report, report.render(self.contract, list(reversed(complete_rows())),
                         context(), self.report["host"], "development"))

    def test_host_description_is_observational_and_every_provenance_field_is_bound(self):
        changed = copy.deepcopy(self.report)
        changed["host"]["cpu_model"] = "another observed CPU"
        report.validate_report(self.contract, changed, "b" * 64)
        self.assertEqual(changed["workloads"], self.report["workloads"])
        for field, replacement in (("probe_sha256", "a" * 64), ("source_tree_sha256", "a" * 64),
                                   ("git_revision", "main"), ("profile", "release"), ("pid", 1),
                                   ("toolchain", {"rustc": "different", "cargo": "different"})):
            changed = copy.deepcopy(self.report)
            changed["context"][field] = replacement
            with self.subTest(field=field):
                with self.assertRaises(ValueError):
                    report.validate_report(self.contract, changed, "b" * 64)
        changed = copy.deepcopy(self.report)
        changed["workloads"][0]["identity_sha256"] = "a" * 64
        with self.assertRaises(ValueError): report.validate_report(self.contract, changed, "b" * 64)

    def test_false_promotions_wrong_receipts_and_clean_revision_are_refused(self):
        for field, replacement in (("strategy", {"native_aot": "verified"}),
                                   ("protocol", {"outliers": "discard"}), ("extra", 1)):
            changed = copy.deepcopy(self.report); changed[field] = replacement
            with self.subTest(field=field):
                with self.assertRaises(ValueError): report.validate_report(self.contract, changed, "b" * 64)
        changed = copy.deepcopy(self.report); changed["capture"] = "clean-source"
        with self.assertRaises(ValueError): report.validate_report(self.contract, changed, "b" * 64)
        changed = complete_rows(); changed[0]["counters"]["accepted_events"] = 1
        with self.assertRaises(ValueError): report.render_workloads(changed, report.ROUTES)
        changed = complete_rows(); changed[0]["counters"]["selected_fixture_bytes"] = 0
        with self.assertRaises(ValueError): report.render_workloads(changed, report.ROUTES)

    def test_contract_accepts_only_exact_workload_and_resource_protocol(self):
        import tempfile
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "contract.json"
            for status in ("measurement-ready", "verified-hosted-scalar-baseline"):
                value = report.contract_value(status)
                report.write_json(path, value)
                self.assertEqual(report.load_contract(path), value)
            for field, replacement in (("target", "aarch64-unknown-linux-gnu"), ("status", "done"),
                                       ("probe", {"sha256": "stale"}), ("workloads", []),
                                       ("identity_fields", []), ("resource_model", {"memory": "RSS"})):
                changed = copy.deepcopy(self.contract); changed[field] = replacement
                report.write_json(path, changed)
                with self.subTest(field=field):
                    with self.assertRaises(ValueError): report.load_contract(path)
            report.write_json(path, self.contract)
            path.write_bytes(path.read_bytes() + b"\n")
            with self.assertRaises(ValueError): report.load_contract(path)


if __name__ == "__main__":
    unittest.main()
