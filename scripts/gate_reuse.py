"""Validated command observations from this gate's completed worker receipts."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import sys

import test_gate_partitions as gate

ROOT = Path(__file__).resolve().parents[1]


def commands(source):
    joined = re.sub(r"\\\n\s*", " ", source)
    result = {}
    for line in joined.splitlines():
        if not line.startswith("run_step "):
            continue
        words = shlex.split(line)
        name = words[1]
        gate.require(name not in result, "duplicate canonical command")
        result[name] = words[2:]
    return result


def normalize(argv):
    if argv and argv[0].startswith(str(ROOT) + "/"):
        argv = [str(Path(argv[0]).relative_to(ROOT)), *argv[1:]]
    return argv


def collect(evidence, contract, source):
    gate.require(source.get("run", {}).get("id") and source["run"]["attempt"],
                 "same-run reuse requires an explicit gate run and attempt")
    observed = {}
    receipts = {}
    for name in gate.WORKERS:
        value = json.loads((evidence / "gate-partitions" / f"{name}.json").read_text())
        gate.validate_receipt(value, name, evidence, source)
        receipts[name] = value
        for step in value["steps"]:
            gate.require(step["name"] not in observed, "duplicate completed command")
            observed[step["name"]] = step
    canonical = commands((ROOT / "scripts/test-gate.sh").read_text())
    selected = {}

    def use(identity, producer):
        gate.require(producer in observed and producer in canonical, "required producer did not complete")
        argv = canonical[producer]
        target = str(Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target"))).resolve())
        bindings = {"$cargo_target_dir": target, "$evidence": str(evidence.resolve())}
        expanded = []
        for argument in argv:
            for variable, value in bindings.items():
                if argument == variable or argument.startswith(variable + "/"):
                    argument = value + argument[len(variable):]
            expanded.append(argument)
        gate.require(all("$" not in argument for argument in expanded), "unresolved producer command")
        selected[identity] = {"producer": producer, "command": shlex.join(expanded),
                              "log": str(evidence / "logs" / f"{producer}.log"),
                              "log_sha256": observed[producer]["log_sha256"]}

    for owner in contract["owners"]:
        candidates = [name for name, argv in canonical.items() if normalize(argv) == [owner["owner_command"]]]
        gate.require(len(candidates) == 1, "owner command has no unique exact canonical producer")
        use("owner-" + owner["id"].removeprefix("std."), candidates[0])
    for owner in contract["owners"]:
        for case in owner["cases"]:
            if case["kind"] == "runtime":
                continue
            argv = shlex.split(case["command"])
            candidates = [name for name, command in canonical.items()
                          if name in observed and normalize(command) == argv]
            if len(argv) == 1 and argv[0].startswith("scripts/") and len(candidates) == 1:
                identity = "case-command-" + hashlib.sha256(case["command"].encode()).hexdigest()[:12]
                use(identity, candidates[0])
    build = ["cargo", "build", "-p", "tondo-conformance", "-p", "tondo-reference-adapter", "--bins", "--locked"]
    gate.require(canonical.get("conformance-build") == build, "adapter build inputs differ")
    use("draft-adapter-build", "conformance-build")
    expected_validate = shlex.split(contract["runner"]["validate_command"])
    gate.require(canonical.get("conformance-validate") == expected_validate, "corpus validation inputs differ")
    use("draft-validate", "conformance-validate")
    expected_run = shlex.split(contract["runner"]["run_command"])
    for option, value in (("--adapter", "$cargo_target_dir/debug/tondo-reference-adapter"),
                          ("--evidence", "$evidence/layer-evidence.json"),
                          ("--output", "$evidence/conformance-result.json")):
        index = expected_run.index(option) + 1
        expected_run[index] = value
    gate.require(canonical.get("conformance-run") == expected_run, "corpus execution inputs differ")
    use("draft-run", "conformance-run")
    # The corpus result is a bound output of the complete foundation worker,
    # not an old report found by path. The normal conformance checker still
    # verifies every case, repetition, manifest, input tree and observation.
    required = {"conformance-result.json", "layer-evidence.json", "layer-evidence-before.json"}
    gate.require(required <= set(receipts["foundation"]["artifacts"]), "corpus/layer output is not bound")
    report = json.loads((evidence / "conformance-result.json").read_text())
    gate.require(report.get("passed") is True and report.get("format") == "tondo-conformance-result-draft/2",
                 "corpus result failed")
    return {"format": "tondo-current-gate-reuse/1", "source": source, "commands": selected}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    result = collect(args.evidence, json.loads(args.contract.read_text()), gate.identity())
    gate.write_json(args.output, result)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, TypeError) as error:
        print(f"current gate reuse: {error}", file=sys.stderr)
        sys.exit(1)
