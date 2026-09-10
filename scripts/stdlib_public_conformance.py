#!/usr/bin/env python3
"""Check reviewed public rows against source-bound draft case observations."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
PLAN = ROOT / "testing/stdlib-meta-reflect-conformance.json"
DIMENSIONS = {
    "positive", "rejection_or_failure", "boundary", "composition", "oracle",
    "public_boundary",
}
SCOPES = {
    "std.meta": "ordinary-tondo-meta",
    "std.reflect": "tondo-vm-hosted-metadata",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read(path):
    return json.loads(Path(path).read_bytes())


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def canonical(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def check_plan(plan):
    require(set(plan) == {"format", "edition", "owners"}, "unknown plan fields")
    require(plan["format"] == "tondo-stdlib-public-conformance-plan/1"
            and plan["edition"] == "0.1", "unsupported plan")
    require([owner["id"] for owner in plan["owners"]] == list(SCOPES),
            "exact public owners are required")
    api = read(ROOT / "testing/stdlib-public-api.json")
    inventory = {row["id"]: row for row in read(ROOT / "testing/inventory.json")["tests"]}
    layer = read(ROOT / "conformance/draft/layers/meta.json")
    attested = {item for case in layer["cases"] for item in case["evidence"]}
    required = set()
    for owner in plan["owners"]:
        require(set(owner) == {"id", "scope", "signatures", "callable_evidence", "requirements"},
                "unknown owner fields")
        name = owner["id"]
        require(owner["scope"] == SCOPES[name], "public scope changed")
        rows = [row for row in api["rows"] if row["owner"] == name]
        require(rows and all(row["status"] == "verified" and not row["missing"] for row in rows),
                "public API implementation gaps")
        require(owner["signatures"] == [
            {"symbol": row["symbol"], "signature": row["signature"]} for row in rows
        ], "public signature plan is stale or incomplete")
        contract = read(ROOT / f"testing/stdlib-{name[4:]}.json")
        expected = sorted(contract["test_matrix"], key=lambda row: row["id"])
        require([row["id"] for row in owner["requirements"]] == [row["id"] for row in expected],
                "owner requirements are incomplete")
        traces = [owner["callable_evidence"]]
        for row, normative in zip(owner["requirements"], expected):
            require(set(row) == {"id", "observables", "dimensions"}, "unknown requirement fields")
            require(row["observables"] == normative["observables"], "owner observables changed")
            require(set(row["dimensions"]) == DIMENSIONS, "missing conformance dimension")
            traces.extend(row["dimensions"].values())
        for evidence in traces:
            require(evidence and evidence == sorted(set(evidence)), "empty or duplicate evidence")
            for test_id in evidence:
                test = inventory.get(test_id)
                require(test is not None and test["kind"] == "rust-test"
                        and test["status"] == "executable", "missing executable test")
                require(test_id in attested, "test is absent from the observed draft layer")
                require(digest(ROOT / test["source"]) == test["source_sha256"], "stale test source")
                required.add(test_id)
    return inventory, layer, required


def check_result(result, inventory, layer, required):
    current = json.loads(subprocess.check_output(
        ["cargo", "run", "-p", "tondo-reliability", "--locked", "--quiet", "--",
         "quality", "provenance", "--root", str(ROOT)], cwd=ROOT,
    ))
    require(result["format"] == "tondo-conformance-result-draft/2"
            and result["passed"] is True and result["lineage"] == "tondo-draft",
            "draft execution did not pass")
    require(result["target"]["name"] == "tondo-vm-hosted", "wrong execution target")
    for key in ["tree_sha256", "input_set_sha256"]:
        require(result[key] == current[key], "stale execution inputs")
    for key, path in [
        ("lineage_manifest_sha256", "conformance/draft/manifest.json"),
        ("manifest_sha256", "conformance/0.1/manifest.json"),
        ("inventory_sha256", "testing/inventory.json"),
    ]:
        require(result[key] == digest(ROOT / path), "stale execution manifest or inventory")
    observed = [item for item in result["case_layers"] if item["id"] == "meta"]
    require(len(observed) == 1, "missing or duplicate meta execution layer")
    observed = observed[0]
    require(observed["manifest_sha256"] == digest(ROOT / "conformance/draft/layers/meta.json"),
            "stale meta layer")
    require([case["id"] for case in observed["cases"]] == [case["id"] for case in layer["cases"]],
            "missing or reordered public cases")
    consumed = set()
    for actual, expected in zip(observed["cases"], layer["cases"]):
        require([item["id"] for item in actual["evidence"]] == expected["evidence"],
                "missing public execution evidence")
        for item in actual["evidence"]:
            require(set(item) == {"id", "source_sha256", "observation_sha256"},
                    "unknown observation fields")
            test = inventory[item["id"]]
            require(item["source_sha256"] == test["source_sha256"], "stale observed test")
            expected_hash = hashlib.sha256(canonical([
                item["id"], item["source_sha256"], "passed",
            ])).hexdigest()
            require(item["observation_sha256"] == expected_hash, "changed test observation")
            consumed.add(item["id"])
        # The conformance runner hashes its ordered compact JSON evidence array.
        encoded = json.dumps(actual["evidence"], ensure_ascii=False, separators=(",", ":")).encode()
        require(actual["observation_sha256"] == hashlib.sha256(encoded).hexdigest(),
                "changed case observation")
    require(required <= consumed, "unobserved public rows")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--plan", action="store_true", help="validate declarations without claiming execution")
    parser.add_argument("--contract", type=Path, default=PLAN)
    parser.add_argument("--result", type=Path, default=Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
                        / "reliability/evidence/conformance-result.json")
    args = parser.parse_args()
    try:
        plan = read(args.contract)
        inventory, layer, required = check_plan(plan)
        if not args.plan:
            check_result(read(args.result), inventory, layer, required)
        counts = ", ".join(f"{owner['id']}: {len(owner['signatures'])} signatures, "
                           f"{len(owner['requirements'])} requirements" for owner in plan["owners"])
        print(f"stdlib public conformance: {'plan only' if args.plan else 'observed'} ({counts})")
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f"stdlib public conformance: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
