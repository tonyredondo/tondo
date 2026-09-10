"""Synthetic prerequisite tests; no fixture is promotion evidence."""

import copy
import unittest

from stdlib_s1a_readiness import CELLS, STAGES, assess


class ReadinessTests(unittest.TestCase):
    def setUp(self):
        names = [f"std.fixture{i}" for i in range(22)]
        self.inputs = {
            "api": {"status": "verified", "owners": [
                {"id": name, "status": "verified", "owner_missing": []} for name in names[:-1]],
                "rows": [{"owner": name, "status": "verified", "missing": []} for name in names[:-1]]},
            "matrix": {"status": "verified", "owners": [
                {"id": name, "stages": {stage: {"status": "verified"} for stage in STAGES}} for name in names],
                "rows": [{"status": "verified"}]},
            "owners": {"owners": [{"id": name, "cells": {
                stage: {"status": "verified"} for stage in CELLS}} for name in names]},
            "fuzz": {"owners": [{"id": name, "status": "verified"} for name in names]},
        }

    def test_eligibility_is_never_promotion(self):
        report = assess(self.inputs)
        self.assertEqual(report["status"], "eligible-for-seal-validation")
        self.assertEqual(report["promotion"], "not-performed")

    def test_open_details_cannot_hide_behind_green_summaries(self):
        mutations = [
            lambda x: x["owners"]["owners"][0]["cells"]["IMPL"].update(status="partial"),
            lambda x: x["owners"]["owners"][0]["cells"]["CONF"].update(status="pending"),
            lambda x: x["fuzz"]["owners"][0].update(status="partial"),
            lambda x: x["matrix"]["rows"][0].update(status="gap"),
            lambda x: x["matrix"]["owners"][0]["stages"]["CONF"].update(status="partial"),
            lambda x: x["api"]["rows"][0]["missing"].append("public-case-call"),
            lambda x: x["api"]["owners"][0]["owner_missing"].append("missing-owner"),
            lambda x: x["api"]["rows"].pop(),
        ]
        for mutate in mutations:
            with self.subTest(mutation=mutate):
                inputs = copy.deepcopy(self.inputs)
                mutate(inputs)
                self.assertEqual(assess(inputs)["status"], "pending")

    def test_missing_empty_and_mismatched_inputs_fail(self):
        for name in self.inputs:
            with self.subTest(name=name):
                inputs = copy.deepcopy(self.inputs)
                inputs[name]["owners"].pop()
                with self.assertRaises(ValueError):
                    assess(inputs)
        for name in ("api", "matrix"):
            inputs = copy.deepcopy(self.inputs)
            inputs[name]["rows"] = []
            with self.assertRaises(ValueError):
                assess(inputs)
        for name, field in (("matrix", "stages"), ("owners", "cells")):
            inputs = copy.deepcopy(self.inputs)
            inputs[name]["owners"][0][field] = {}
            with self.assertRaises(ValueError):
                assess(inputs)
        self.inputs["fuzz"]["owners"][0]["id"] = "std.unknown"
        with self.assertRaises(ValueError):
            assess(self.inputs)


if __name__ == "__main__":
    unittest.main()
