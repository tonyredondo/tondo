"""Check the required evidence inside an integrity-checked S1A bundle.

This validates the supplied records and their bindings. It does not rerun the
recorded commands or establish that an unpublished language is release-ready.
"""

import argparse
import hashlib
import json
from pathlib import Path


def digest(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_payload(bundle, contract_path, seal, manifest):
    contract_bytes = contract_path.read_bytes()
    contract = json.loads(contract_bytes)
    paths = {entry["path"] for entry in manifest["files"]}
    required = {"inputs/" + path for path in contract["required_inputs"]}
    required.update("evidence/" + Path(item["path"]).name
                    for item in contract["required_evidence"])
    required.update({"metadata/commands.json", "metadata/verification.json",
                     "evidence/conformance-result.json", "evidence/layer-evidence.json",
                     "distribution/tondo-std-0.1.tar"})
    require(required <= paths, "bundle omits required inputs or evidence: "
            + ", ".join(sorted(required - paths)))

    def data(path):
        require(path in paths, f"unindexed required payload: {path}")
        return (bundle / path).read_bytes()

    def record(path):
        return json.loads(data(path))

    require(data("inputs/testing/stdlib-s1a-seal.json") == contract_bytes,
            "bundled seal contract differs from the verification contract")
    for field in ("revision", "source_tree_sha256", "payload_sha256"):
        require(manifest[field] == seal[field], f"manifest {field} differs from seal")
    verification = record("metadata/verification.json")
    require(verification["format"] == "tondo-stdlib-s1a-verification/1"
            and verification["task"] == seal["task"]
            and verification["edition"] == seal["edition"]
            and verification["phase"] == seal["phase"]
            and verification["revision"] == seal["revision"]
            and verification["source_tree_sha256"] == seal["source_tree_sha256"]
            and verification["contract_sha256"] == digest(contract_bytes)
            and verification["claims"] == contract["invariants"]["claims"],
            "verification record differs from the seal or contract")
    commands = record("metadata/commands.json")
    require(commands == verification["checks"], "verification command records differ")
    require([item["command"] for item in commands] == contract["required_checks"],
            "required checks are missing, reordered or substituted")
    for index, item in enumerate(commands, start=1):
        identifier = f"check-{index:02}"
        require(item["id"] == identifier and item["status"] == "passed"
                and item["log"] == f"commands/{identifier}.log",
                "required check identity or status differs")
        require(digest(data(item["log"])) == item["log_sha256"],
                "required check transcript hash differs")

    for expected in contract["required_evidence"]:
        report = record("evidence/" + Path(expected["path"]).name)
        require(report["format"] == expected["format"], "evidence format differs")
        if expected["status"] is not None:
            require(report.get("status") == expected["status"], "evidence status differs")
        for field, value in ((expected["revision_field"], seal["revision"]),
                             (expected["tree_field"], seal["source_tree_sha256"])):
            if field:
                require(report.get(field) == value, "evidence revision or tree differs")

    api = record("inputs/testing/stdlib-public-api.json")
    api_required = contract["invariants"]["public_api"]
    api_owners = {owner["id"] for owner in api["owners"]}
    require(api["status"] == "verified" and api["summary"]["gaps"] == 0
            and api["summary"]["signatures"] == api_required["signatures"]
            and api["summary"]["verified"] == api_required["signatures"]
            and len(api["rows"]) == api_required["signatures"]
            and len(api_owners) == len(api["owners"]) == api_required["owners"]
            and len({row["id"] for row in api["rows"]}) == len(api["rows"])
            and all(row["owner"] in api_owners for row in api["rows"])
            and all(row["status"] == "verified" and not row["missing"] for row in api["rows"])
            and all(owner["status"] == "verified" for owner in api["owners"]),
            "bundled public API has gaps")
    matrix = record("inputs/testing/stdlib-matrix.json")
    matrix_required = contract["invariants"]["matrix"]
    matrix_owners = {owner["id"] for owner in matrix["owners"]}
    stages = {"SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC"}
    require(matrix["status"] == "verified"
            and matrix["summary"]["owners"] == matrix_required["owners"]
            and matrix["summary"]["requirements"] == matrix_required["requirements"]
            and matrix["summary"]["rows"] == matrix_required["rows"]
            and matrix["summary"]["open_rows"] == 0
            and len(matrix["rows"]) == matrix_required["rows"]
            and len(matrix_owners) == len(matrix["owners"]) == matrix_required["owners"]
            and api_owners <= matrix_owners
            and len({row["id"] for row in matrix["rows"]}) == len(matrix["rows"])
            and all(row["owner"] in matrix_owners for row in matrix["rows"])
            and all(row["status"] == "verified" for row in matrix["rows"])
            and all(set(owner["stages"]) == stages for owner in matrix["owners"])
            and all(cell["status"] in ("verified", "not-applicable")
                    for owner in matrix["owners"] for cell in owner["stages"].values()),
            "bundled normative matrix has open cells")
    owners = record("inputs/testing/stdlib-owner-evidence.json")
    require(owners["status"] == "promoted-evidence"
            and len(owners["owners"]) == contract["invariants"]["fuzz"]["owners"]
            and {owner["id"] for owner in owners["owners"]} == matrix_owners
            and all(owner["cells"]["FUZZ"]["status"] == "verified"
                    and owner["cells"]["FUZZ"]["reason"] is None for owner in owners["owners"]),
            "bundled fuzz evidence is not promoted")

    conformance = record("evidence/stdlib-conformance.json")
    require(conformance.get("promotion") == "promoted"
            and conformance.get("public_row_coverage") == "verified",
            "declared-case observations do not establish public conformance")
    require(conformance["contract_sha256"] == digest(data("inputs/testing/stdlib-conformance.json"))
            and conformance["manifest_sha256"] == digest(data("inputs/conformance/0.1/manifest.json")),
            "conformance contract or manifest binding differs")
    result = record("evidence/conformance-result.json")
    layer = record("evidence/layer-evidence.json")
    suite = record("inputs/conformance/0.1/manifest.json")
    require(conformance["full_suite"]["result_sha256"] == digest(data("evidence/conformance-result.json"))
            and conformance["full_suite"]["passed"] is True
            and conformance["full_suite"]["cases"] == contract["invariants"]["conformance"]["cases"]
            and result["passed"] is True and layer["passed"] is True
            and result["tree_sha256"] == layer["tree_sha256"] == seal["source_tree_sha256"]
            and result["manifest_sha256"] == conformance["manifest_sha256"]
            and result["lineage_manifest_sha256"] == layer["manifest_sha256"]
            == digest(data("inputs/conformance/draft/manifest.json"))
            and result["inventory_sha256"] == layer["inventory_sha256"]
            == digest(data("inputs/testing/inventory.json"))
            and result["input_set_sha256"] == layer["input_set_sha256"]
            and sorted(case["id"] for case in result["cases"]) == sorted(case["id"] for case in suite["cases"]),
            "conformance results or layer evidence differ")
    for command in conformance["commands"]:
        path = "evidence/stdlib-conformance-logs/" + Path(command["log"]).name
        require(command["status"] == "passed" and digest(data(path)) == command["log_sha256"],
                "conformance transcript differs")

    distribution = record("evidence/stdlib-distribution.json")
    require(distribution.get("promotion") == "promoted"
            and distribution["inputs"]["source_revision"] == seal["revision"]
            and distribution["inputs"]["source_dirty"] is False
            and distribution["inputs"]["binary_source_provenance"] == "verified"
            and distribution["byte_identical"] is True
            and distribution["clean_source_workspaces"] == 2
            and distribution["public_release"] is False
            and distribution["archive_sha256"] == digest(data("distribution/tondo-std-0.1.tar")),
            "distribution lacks promoted source-bound evidence")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bundle", "contract", "seal", "manifest"):
        parser.add_argument("--" + name, type=Path, required=True)
    args = parser.parse_args()
    verify_payload(args.bundle, args.contract, json.loads(args.seal.read_bytes()),
                   json.loads(args.manifest.read_bytes()))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"stdlib S1A payload: {error}") from error
