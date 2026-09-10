"""Report S1A prerequisites; eligibility is not observed conformance or a seal."""

import argparse
import hashlib
import json
from pathlib import Path


INPUTS = {
    "api": "testing/stdlib-public-api.json",
    "matrix": "testing/stdlib-matrix.json",
    "owners": "testing/stdlib-owner-evidence.json",
    "fuzz": "testing/stdlib-fuzz.json",
}
CELLS = {"SPEC", "IMPL", "HOST", "MODEL", "TEST", "FUZZ", "PERF", "CONF", "DOC"}
STAGES = {"SPEC", "IMPL/HOST", "MODEL/TEST/FUZZ", "PERF", "CONF", "DOC"}


def assess(inputs):
    """Keep every open component visible, independently of summary labels."""
    blockers = []
    owner_ids = None
    for name, count in (("matrix", 22), ("owners", 22), ("fuzz", 22), ("api", 21)):
        rows = inputs[name]["owners"]
        ids = {row["id"] for row in rows}
        if len(rows) != count or len(ids) != count:
            raise ValueError(f"{name}: invalid owner inventory")
        if name == "matrix":
            owner_ids = ids
        elif name != "api" and ids != owner_ids:
            raise ValueError(f"{name}: owner identities differ from matrix")
        elif name == "api" and not ids < owner_ids:
            raise ValueError("api: unknown owner identities")

    api = inputs["api"]
    signatures = api["rows"]
    if not signatures:
        raise ValueError("api: empty callable inventory")
    if any(not any(row["owner"] == owner["id"] for row in signatures) for owner in api["owners"]):
        blockers.append("public-api: owner without callable signatures")
    if (api["status"] != "verified"
            or any(row["status"] != "verified" or row["missing"] for row in signatures)
            or any(owner["status"] != "verified" or owner["owner_missing"] for owner in api["owners"])):
        blockers.append("public-api: incomplete traceability")

    matrix = inputs["matrix"]
    if not matrix["rows"]:
        raise ValueError("matrix: empty requirement inventory")
    if matrix["status"] != "verified" or any(row["status"] != "verified" for row in matrix["rows"]):
        blockers.append("matrix: open rows")
    for owner in matrix["owners"]:
        if set(owner["stages"]) != STAGES:
            raise ValueError(f"{owner['id']}: incomplete matrix stages")
        for stage, cell in owner["stages"].items():
            if cell["status"] not in ("verified", "not-applicable"):
                blockers.append(f"{owner['id']}: matrix {stage} {cell['status']}")
    for owner in inputs["owners"]["owners"]:
        if set(owner["cells"]) != CELLS:
            raise ValueError(f"{owner['id']}: incomplete owner cells")
        for stage, cell in owner["cells"].items():
            inapplicable = stage in ("HOST", "PERF") and cell["status"] == "not-applicable" and cell.get("reason")
            if cell["status"] != "verified" and not inapplicable:
                blockers.append(f"{owner['id']}: {stage} {cell['status']}")
    for owner in inputs["fuzz"]["owners"]:
        if owner["status"] != "verified":
            blockers.append(f"{owner['id']}: fuzz route {owner['status']}")
    blockers = sorted(set(blockers))
    return {
        "format": "tondo-stdlib-s1a-readiness/1",
        "status": "pending" if blockers else "eligible-for-seal-validation",
        "promotion": "not-performed",
        "blockers": blockers,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--require-ready", action="store_true")
    args = parser.parse_args()
    inputs, hashes = {}, {}
    try:
        for name, relative in INPUTS.items():
            raw = (args.root / relative).read_bytes()
            inputs[name] = json.loads(raw)
            hashes[relative] = hashlib.sha256(raw).hexdigest()
        report = assess(inputs)
    except (OSError, ValueError, KeyError, TypeError) as error:
        parser.exit(1, f"S1A readiness: invalid prerequisite: {error}\n")
    report["inputs"] = hashes
    text = json.dumps(report, sort_keys=True, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(text)
    print(f"S1A readiness: {report['status']} ({len(report['blockers'])} open prerequisites; no promotion)")
    if args.require_ready and report["blockers"]:
        parser.exit(3, "S1A seal blocked by open prerequisites\n")


if __name__ == "__main__":
    main()
