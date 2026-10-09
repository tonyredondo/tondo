"""Disjoint full-gate plans and source-bound worker evidence."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
WORKERS = ("foundation", "native", "runtime", "stdlib")
PARTITIONS = (*WORKERS, "final")
FORMAT = "tondo-test-gate-partition/1"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            value.update(block)
    return value.hexdigest()


def plans(source=None):
    source = (ROOT / "scripts/test-gate.sh").read_text() if source is None else source
    names = re.findall(r"^run_step ([a-z0-9-]+)", source, re.M)
    require(names and len(names) == len(set(names)), "empty or duplicate canonical gate steps")
    require("stdlib-public-api-audit" in names, "missing final dependency boundary")
    final = names.index("stdlib-public-api-audit")
    result = {name: [] for name in PARTITIONS}
    for index, name in enumerate(names):
        if index >= final:
            partition = "final"
        elif name.startswith("native-"):
            partition = "native"
        elif name.startswith(("stdlib-async-group-", "stdlib-channel-", "stdlib-sync-", "stdlib-executor-")):
            partition = "runtime"
        elif name.startswith("stdlib-"):
            partition = "stdlib"
        else:
            partition = "foundation"
        result[partition].append(name)
    require(all(result.values()), "empty partition")
    return result


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def identity():
    require(not command("git", "status", "--porcelain"), "gate source must be clean")
    return {
        "revision": command("git", "rev-parse", "HEAD"),
        "git_tree": command("git", "rev-parse", "HEAD^{tree}"),
        "rustc": command("rustc", "--version", "--verbose"),
        "cargo": command("cargo", "--version"),
        "inputs": {key: os.environ.get(key, "") for key in (
            "TONDO_TEST_TARGET", "TONDO_TEST_SEED", "CARGO_INCREMENTAL",
            "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")},
    }


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def receipt(partition, evidence, before):
    current = identity()
    require(current == json.loads(before.read_text()), "source or toolchain changed during worker")
    completed = (evidence / "gate-partitions" / f"{partition}.steps").read_text().splitlines()
    require(completed == plans()[partition], "missing, duplicated or reordered worker steps")
    entries = []
    for name in completed:
        log = evidence / "logs" / f"{name}.log"
        require(log.is_file(), f"missing execution log: {name}")
        entries.append({"name": name, "log_sha256": digest(log)})
    return {"format": FORMAT, "partition": partition, "status": "passed",
            "source": current, "steps": entries}


def validate_receipt(value, partition, root, expected_source):
    require(type(value) is dict and set(value) == {"format", "partition", "status", "source", "steps"},
            "receipt fields")
    require(value["format"] == FORMAT and value["partition"] == partition
            and value["status"] == "passed" and value["source"] == expected_source, "worker source/status")
    require(type(value["steps"]) is list
            and [step["name"] for step in value["steps"]] == plans()[partition], "worker completeness")
    for step in value["steps"]:
        require(set(step) == {"name", "log_sha256"}, "step fields")
        require(digest(root / "logs" / f"{step['name']}.log") == step["log_sha256"], "execution log changed")


def merge(inputs, output, expected_source):
    received = {}
    roots = []
    for file in sorted(inputs.rglob("gate-partitions/*.json")):
        value = json.loads(file.read_text())
        name = value.get("partition")
        require(name in WORKERS and name not in received, "unknown or duplicate worker receipt")
        root = file.parent.parent
        validate_receipt(value, name, root, expected_source)
        received[name] = value
        roots.append(root)
    require(set(received) == set(WORKERS), "missing worker")
    # Validate all collisions before publishing any merged evidence.
    files = {}
    for root in roots:
        for file in sorted(root.rglob("*")):
            require(not file.is_symlink(), "symlink in evidence")
            if not file.is_file():
                continue
            relative = file.relative_to(root)
            if relative in files:
                require(digest(file) == digest(files[relative]), f"conflicting evidence: {relative}")
            else:
                files[relative] = file
    for relative, file in files.items():
        destination = output / relative
        if destination.exists():
            require(destination.is_file() and digest(destination) == digest(file),
                    f"existing evidence differs: {relative}")
    for relative, file in files.items():
        destination = output / relative
        if not destination.exists():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(file, destination)
    return received


def verify_fast(inputs, revision):
    files = list(inputs.rglob("summary.json"))
    require(len(files) == 1, "exactly one completed fast gate is required")
    value = json.loads(files[0].read_text())
    require(value["format"] == "tondo-fast-gate-evidence/1"
            and value["head"] == revision
            and value["scope"] in {"documentation", "impacted", "evaluation"}
            and type(value["full_required"]) is int and value["full_required"] == 0,
            "fast gate source or tier differs")
    require((files[0].parent / "plan.txt").read_text().strip(), "missing executed fast plan")
    return value["scope"]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("plan", "before", "receipt", "merge", "verify", "verify-fast"))
    parser.add_argument("--partition", choices=PARTITIONS)
    parser.add_argument("--evidence", type=Path)
    parser.add_argument("--before", type=Path)
    parser.add_argument("--inputs", type=Path)
    args = parser.parse_args()
    if args.mode == "plan":
        value = plans()
        if args.partition:
            print("\n".join(value[args.partition]))
        else:
            print(json.dumps(value, indent=2))
    elif args.mode == "before":
        write_json(args.before, identity())
    elif args.mode == "receipt":
        write_json(args.evidence / "gate-partitions" / f"{args.partition}.json",
                   receipt(args.partition, args.evidence, args.before))
    elif args.mode == "merge":
        merge(args.inputs, args.evidence, identity())
    elif args.mode == "verify-fast":
        scope = verify_fast(args.inputs, command("git", "rev-parse", "HEAD"))
        print(f"current fast gate verified: {scope}")
    else:
        source = identity()
        for name in PARTITIONS:
            value = json.loads((args.evidence / "gate-partitions" / f"{name}.json").read_text())
            validate_receipt(value, name, args.evidence, source)
        print(f"complete test gate: {sum(map(len, plans().values()))} unique source-bound steps verified")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        print(f"test gate partitions: {error}", file=sys.stderr)
        sys.exit(1)
