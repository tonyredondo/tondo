"""Compile the Cargo workspace once and execute every libtest suite in a shard."""

import argparse
import concurrent.futures
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time

import test_gate_partitions as gate

ROOT = Path(__file__).resolve().parents[1]
FORMAT = "tondo-rust-suite-execution/1"
SUMMARY = re.compile(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out; finished in ([\d.]+)s$", re.M)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    return gate.digest(path)


def target_id(target):
    path = Path(target["src_path"]).resolve().relative_to(ROOT)
    return f"{path.as_posix()}:{','.join(target['kind'])}:{target['name']}"


def discover(metadata, messages):
    packages = {package["id"]: package for package in metadata["packages"]
                if package["id"] in metadata["workspace_members"]}
    expected = {target_id(target) for package in packages.values() for target in package["targets"]
                if target["kind"] != ["custom-build"]}
    require(any(m.get("reason") == "build-finished" and m["success"] for m in messages), "Cargo build did not succeed")
    suites = {}
    bins = {}
    for item in messages:
        if item.get("reason") != "compiler-artifact" or not item.get("executable"):
            continue
        package = packages.get(item["package_id"])
        if package is None:
            continue
        if not item["profile"]["test"]:
            if item["target"]["kind"] == ["bin"]:
                bins.setdefault(package["id"], {})[item["target"]["name"]] = item["executable"]
            continue
        identity = target_id(item["target"])
        require(identity not in suites, "duplicate Cargo test target")
        executable = Path(item["executable"])
        executable.resolve().relative_to(Path(metadata["target_directory"]).resolve())
        require(executable.is_file() and not executable.is_symlink(), "missing test executable")
        suites[identity] = {"id": identity, "package": package["name"],
                            "directory": str(Path(package["manifest_path"]).parent),
                            "executable": str(executable), "binary_sha256": digest(executable),
                            "package_id": package["id"]}
    require(set(suites) == expected, "Cargo target discovery is incomplete or unexpected")
    for suite in suites.values():
        suite["binaries"] = bins.get(suite.pop("package_id"), {})
    return sorted(suites.values(), key=lambda suite: suite["id"])


def environment(suite):
    result = os.environ.copy()
    threads = result.get("RUST_TEST_THREADS", "4")
    require(threads.isdigit() and 1 <= int(threads) <= 4, "suite test threads must be between one and four")
    result["RUST_TEST_THREADS"] = threads
    result["CARGO_MANIFEST_DIR"] = suite["directory"]
    result["CARGO_PKG_NAME"] = suite["package"]
    result.update({f"CARGO_BIN_EXE_{name}": path for name, path in suite["binaries"].items()})
    return result


def test_names(output):
    names = [match[1] for match in re.finditer(r"^(.+): test$", output, re.M)]
    benchmarks = re.findall(r"^(.+): benchmark$", output, re.M)
    require(not benchmarks, "benchmark harness needs an explicit execution protocol")
    require(len(names) == len(set(names)), "duplicate listed test")
    require(all(name and '\n' not in name for name in names), "invalid test name")
    return sorted(names)


def schedule(suites, weights, count):
    require(type(count) is int and 1 <= count <= 4, "suite concurrency must be between one and four")
    require(len({suite['id'] for suite in suites}) == len(suites), "duplicate scheduled suite")
    require(all(type(v) in (int, float) and 0 <= v < float('inf') for v in weights.values()), "invalid duration hint")
    shards = [[] for _ in range(count)]
    totals = [0.0] * count
    for suite in sorted(suites, key=lambda s: (-weights.get(s["id"], 1.0), s["id"])):
        shard = min(range(count), key=lambda i: (totals[i], i))
        shards[shard].append(suite)
        totals[shard] += weights.get(suite["id"], 1.0)
    return shards


def check_result(suite, output, status):
    summaries = SUMMARY.findall(output)
    require(status == 0 and len(summaries) == 1, "suite failed or has no single terminal summary")
    summary = summaries[0]
    require(summary[0] == "ok" and all(int(n) == 0 for n in summary[2:6]), "failed, ignored, measured or filtered tests")
    passed = sorted(match[1] for match in re.finditer(r"^test (.+) \.\.\. ok$", output, re.M))
    require(passed == suite["tests"] and int(summary[1]) == len(passed), "suite test coverage differs from discovery")
    return len(passed)


def verify(suites, results, output):
    ids = [row["id"] for row in results]
    require(len(ids) == len(set(ids)) and set(ids) == {s["id"] for s in suites}, "missing or duplicated suite result")
    for suite in suites:
        row = next(row for row in results if row["id"] == suite["id"])
        log = output / row["log"]
        require(row["status"] == "passed" and digest(log) == row["log_sha256"], "suite log/status changed")
        require(check_result(suite, log.read_text(), 0) == row["passed"], "suite counter differs")


def run(output, count):
    require(type(count) is int and 1 <= count <= 4, "suite concurrency must be between one and four")
    source = gate.identity()
    require(not (output / "summary.json").exists(), "remove or archive this run's old summary before a new execution")
    output.mkdir(parents=True, exist_ok=True)
    build = output / "build.jsonl"
    with build.open("w") as handle:
        subprocess.run(["cargo", "test", "--workspace", "--all-targets", "--locked", "--no-run", "--message-format=json"],
                       cwd=ROOT, stdout=handle, check=True)
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=ROOT))
    messages = [json.loads(line) for line in build.read_text().splitlines() if line.startswith("{")]
    suites = discover(metadata, messages)
    for suite in suites:
        listing = subprocess.check_output([suite["executable"], "--list", "--format=terse"],
                                          cwd=suite["directory"], env=environment(suite), text=True)
        suite["tests"] = test_names(listing)
    weights_path = ROOT / "testing/rust-suite-durations.json"
    hints = json.loads(weights_path.read_text())
    require(hints.get("format") == "tondo-rust-suite-duration-hints/1"
            and type(hints.get("seconds")) is dict, "invalid duration hint register")
    weights = hints["seconds"]
    shards = schedule(suites, weights, count)
    gate.write_json(output / "plan.json", {"source": source, "suites": suites,
                                          "shards": [[s["id"] for s in shard] for shard in shards],
                                          "weights_sha256": digest(weights_path)})

    def execute(index):
        results = []
        for suite in shards[index]:
            name = hashlib.sha256(suite["id"].encode()).hexdigest()[:24] + ".log"
            log = output / name
            started = time.monotonic()
            error = None
            passed = 0
            try:
                require(digest(Path(suite["executable"])) == suite["binary_sha256"], "test executable changed after build")
                with log.open("w") as handle:
                    status = subprocess.run([suite["executable"], "--test", "--color=never"],
                                            cwd=suite["directory"], env=environment(suite),
                                            stdin=subprocess.DEVNULL, stdout=handle, stderr=subprocess.STDOUT).returncode
                passed = check_result(suite, log.read_text(), status)
            except (OSError, ValueError, subprocess.SubprocessError) as failure:
                error = str(failure)
                if not log.exists():
                    log.write_text(error + "\n")
            results.append({"id": suite["id"], "shard": index, "status": "passed" if error is None else "failed",
                            "passed": passed, "error": error, "log": name, "log_sha256": digest(log),
                            "seconds": time.monotonic() - started})
        return results

    started = time.monotonic()
    with concurrent.futures.ThreadPoolExecutor(max_workers=count) as executor:
        results = [row for shard in executor.map(execute, range(count)) for row in shard]
    # Replay complete, actual suite logs in stable order for the existing layer
    # attestation parser. Parallel child output is never interleaved or invented.
    for row in sorted(results, key=lambda result: result["id"]):
        print(f"Running {row['id']}")
        print((output / row["log"]).read_text(), end="", flush=True)
    require(gate.identity() == source, "source/build inputs changed during suite execution")
    summary = {"format": FORMAT, "source": source, "shards": count, "status": "passed",
               "suites": len(suites), "tests": sum(row["passed"] for row in results),
               "execution_seconds": time.monotonic() - started, "results": results,
               "plan_sha256": digest(output / "plan.json")}
    try:
        verify(suites, results, output)
    except (OSError, ValueError) as error:
        summary.update(status="failed", error=str(error))
        gate.write_json(output / "summary.json", summary)
        raise
    gate.write_json(output / "summary.json", summary)
    print(f"Rust suites verified: {len(suites)} suites, {summary['tests']} tests, {count} shards")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--shards", type=int, default=4)
    args = parser.parse_args()
    run(args.output, args.shards)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(f"Rust suites: {error}", file=sys.stderr)
        sys.exit(1)
