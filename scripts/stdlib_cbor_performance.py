#!/usr/bin/env python3
"""Validate and render the target-qualified CBOR scalar-kernel campaign."""

import argparse
import hashlib
import json
import math
import pathlib
import re
import subprocess
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parent.parent
PROBE = "crates/tondo-stdlib/tests/cbor_performance.rs"
ROUTES = {
    "parse-keys-small": "materialized-parse",
    "parse-maps-medium": "materialized-parse",
    "parse-maps-large": "materialized-parse",
    "parse-indefinite": "materialized-parse",
    "parse-view-large": "borrowed-parse-view",
    "encode-maps-large": "materialized-encode",
    "deterministic-maps-large": "deterministic-encode",
    "deterministic-floats-tags": "deterministic-encode",
    "reader-indefinite": "stream-reader-events",
    "writer-indefinite": "stream-writer-events",
    "reject-depth": "adversarial-reject",
    "reject-scalar": "adversarial-reject",
    "reject-events": "adversarial-reject",
    "reject-simple": "adversarial-reject",
    "reject-key-collision": "adversarial-reject",
}
ERRORS = {
    "reject-depth": "LimitExceeded", "reject-scalar": "LimitExceeded",
    "reject-events": "LimitExceeded", "reject-simple": "InvalidSimpleValue",
    "reject-key-collision": "DeterministicKeyCollision",
}
COUNTERS = {
    "input_bytes", "output_bytes", "operations", "bytes_copied", "allocations",
    "logical_memory_bytes", "nodes", "depth", "arrays", "maps", "map_pairs",
    "tags", "chunks", "event_count", "adversarial_rejections",
    "terminal_open_streams", "native_live_handles",
}
IDENTITY = [
    "suite", "workload_id", "probe_sha256", "source_tree_sha256", "target",
    "backend", "profile", "toolchain", "flags", "git_revision",
]
PROTOCOL = {
    "clock": "monotonic", "warmup_iterations": 3, "measurement_repetitions": 9,
    "independent_processes": 3, "minimum_sample_count": 27, "batch_operations": 16,
    "fixtures": "deterministic-authored-wire-and-reference-values",
    "outliers": "report-not-delete",
    "sample_coordinates": "process-1-through-3-and-repetition-0-through-8",
    "fixture_setup": "excluded-from-timed-latency; retained-fixture-identities-included-in-resource-model",
}
STRATEGY = {
    "stdlib_kernel": "direct-rust-scalar-kernel-baseline", "compiler_api": "not-claimed",
    "hosted_vm": "not-claimed-no-cbor-bridge", "native_runtime_abi": "not-measured",
    "native_aot": "not-claimed", "simd": "not-measured-no-optimized-route",
    "multiversion_dispatch": "not-claimed", "code_size": "not-measured", "selection": "scalar-only",
}
RESOURCE_MODEL = {
    "copies": "selected-operation-payload-transport; excludes-private-temporary-copies",
    "allocations": "retained-fixture-and-operation-payload-result-identities; not-allocator-calls",
    "logical_memory": "modeled-retained-fixture-plus-operation-payload-result-storage; not-RSS-or-allocator-peak",
    "excluded": ["oracle-temporaries", "private-parser-worklists", "capacity-slack",
                 "sorting-metadata", "other-workloads", "OS-allocator-and-native-runtime"],
    "terminal_open_streams": "finish-and-lexical-drop-invariant; independently-checked-Closed-states",
    "native_live_handles": "unmeasured-null; no-native-bridge",
    "layout": {"target": "x86_64-unknown-linux-gnu", "value_bytes": 32, "event_bytes": 32, "path_bytes": 16},
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    data = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(data).hexdigest()


def validate_counters(counters, workload_id):
    require(set(counters) == COUNTERS, "counter fields")
    require(counters["native_live_handles"] is None, "native handles are unmeasured")
    require(all(type(value) is int and 0 <= value <= 1_048_576 for key, value in counters.items()
                if key != "native_live_handles"), "invalid logical counter")
    require(0 < counters["input_bytes"] <= 4096 and counters["operations"] == 16,
            "input or batch bound")
    require(counters["nodes"] <= 128 and counters["depth"] <= 8, "reference domain")
    require(counters["allocations"] > 0 and counters["logical_memory_bytes"] > 0,
            "missing resource model")
    require(counters["terminal_open_streams"] == 0, "open reader or writer")
    rejecting = workload_id in ERRORS
    require(counters["adversarial_rejections"] == (16 if rejecting else 0), "rejection count")
    emitting = ROUTES[workload_id] in {"materialized-encode", "deterministic-encode", "stream-writer-events"}
    require((counters["output_bytes"] > 0) == emitting, "partial or missing output")
    require(counters["bytes_copied"] == 0 if rejecting else counters["bytes_copied"] > 0,
            "payload transport boundary")
    streams = ROUTES[workload_id] in {"stream-reader-events", "stream-writer-events"}
    require(counters["event_count"] == (14 if streams else 0), "owned event boundary")


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b"\n") and not raw.endswith(b"\n\n"), "one final LF required")
    require(b"\r" not in raw and all(line.rstrip() == line for line in raw.splitlines()), "whitespace")
    contract = json.loads(raw)
    require(contract["format"] == "tondo-stdlib-cbor-performance/1", "contract format")
    require(contract["task"] == "STD-CBOR-PERF-001" and contract["owner"] == "std.cbor", "owner")
    require(contract["edition"] == "0.1" and contract["phase"] == "STD-0.1B", "edition or phase")
    require(contract["status"] in {"measurement-ready", "verified-stdlib-kernel-baseline"}, "status")
    require(contract["target"] == "x86_64-unknown-linux-gnu" and contract["backend"] == "rust-stdlib-kernel"
            and contract["profile"] == "test", "target, backend or profile")
    require(contract["contract"] == "docs/contracts/stdlib-cbor-performance.md"
            and contract["parent_contract"] == "testing/stdlib-cbor.json", "contract links")
    probe = contract["probe"]
    require(set(probe) == {"path", "test", "sha256"} and probe["path"] == PROBE
            and probe["test"] == "cbor_performance_probe", "probe route")
    require(re.fullmatch(r"[0-9a-f]{64}", probe["sha256"]), "probe hash")
    require(hashlib.sha256((ROOT / PROBE).read_bytes()).hexdigest() == probe["sha256"], "stale probe hash")
    require(contract["protocol"] == PROTOCOL and contract["strategy"] == STRATEGY, "protocol or strategy")
    require(contract["identity_fields"] == IDENTITY, "identity fields")
    require(contract["forbidden_identity"] == ["ambient_environment", "cpu_frequency", "path", "pid", "timestamp"],
            "ambient identity fields")
    require(len(contract["metrics"]) == len(set(contract["metrics"]))
            and set(contract["metrics"]) == COUNTERS - {"input_bytes", "output_bytes", "operations"}
            | {"latency", "tail_latency", "throughput", "dispatch"}, "metric fields")
    require(contract["limits"] == {"fixture_input_bytes": 4096, "reference_nodes": 128,
            "reference_depth": 8, "reference_scalar_bytes": 96}, "oracle limits")
    oracle = contract["oracle"]
    require(oracle == {"kind": "independent-bounded-cbor-wire-model-and-authored-full-errors-events", "sources": [
        "crates/tondo-reliability/src/cbor_model.rs", "crates/tondo-reliability/tests/cbor_models.rs", PROBE,
    ]}, "independent oracle")
    require(all((ROOT / source).is_file() for source in oracle["sources"]), "oracle source missing")
    require(contract["resource_model"] == RESOURCE_MODEL, "resource model boundary")
    require(contract["report"] == "target/reliability/evidence/stdlib-cbor-performance.json", "report path")
    workloads = contract["workloads"]
    require(len(workloads) == 15 and {workload["id"] for workload in workloads} == set(ROUTES), "workload set")
    for workload in workloads:
        name = workload["id"]
        require(set(workload) == {"id", "operation", "size_class", "expected_error", "dispatch", "counters"},
                "workload fields")
        require(workload["operation"] == ROUTES[name] and workload["expected_error"] == ERRORS.get(name), "route oracle")
        size = "large" if "large" in name else "medium" if "medium" in name else "small"
        require(workload["size_class"] == size and workload["dispatch"] == "scalar-fixed-target", "route class")
        validate_counters(workload["counters"], name)
    return contract


def percentile(samples, fraction):
    return samples[math.ceil(len(samples) * fraction) - 1]


def measurements(contract, rows):
    grouped = defaultdict(list)
    for row in rows:
        require(set(row) == {"workload_id", "operation", "process", "repetition", "nanos", "dispatch", "counters"},
                "sample fields")
        require(row["workload_id"] in ROUTES, "unknown workload")
        require(type(row["nanos"]) is int and row["nanos"] > 0, "monotonic sample")
        require(type(row["process"]) is int and 1 <= row["process"] <= 3, "process ordinal")
        require(type(row["repetition"]) is int and 0 <= row["repetition"] < 9, "repetition ordinal")
        grouped[row["workload_id"]].append(row)
    require(set(grouped) == set(ROUTES), "missing workload")
    result = []
    for spec in sorted(contract["workloads"], key=lambda workload: workload["id"]):
        samples = grouped[spec["id"]]
        require(len(samples) == 27, "27 retained samples required")
        require({(row["process"], row["repetition"]) for row in samples}
                == {(process, repetition) for process in range(1, 4) for repetition in range(9)},
                "missing or duplicated process/repetition")
        for row in samples:
            validate_counters(row["counters"], spec["id"])
            require(row["operation"] == spec["operation"] and row["dispatch"] == spec["dispatch"], "sample route")
            require(row["counters"] == spec["counters"], "model counter drift")
        elapsed = sorted(row["nanos"] for row in samples)
        median = percentile(elapsed, 0.5)
        counters = spec["counters"]
        transported = counters["output_bytes"] or counters["input_bytes"]
        result.append({
            "workload_id": spec["id"], "operation": spec["operation"], "size_class": spec["size_class"],
            "expected_error": spec["expected_error"], "dispatch": spec["dispatch"], "sample_count": 27,
            "samples": sorted(({"process": row["process"], "repetition": row["repetition"], "nanos": row["nanos"]}
                               for row in samples), key=lambda row: (row["process"], row["repetition"])),
            "median_ns": median, "p95_ns": percentile(elapsed, 0.95), "p99_ns": percentile(elapsed, 0.99),
            "throughput_bytes_per_second": transported * 16 * 1_000_000_000 // median,
            "throughput_operations_per_second": 16 * 1_000_000_000 // median,
            "counters": counters,
        })
    return result


def identity(report, workload_id):
    return {field: workload_id if field == "workload_id" else report[field] for field in IDENTITY}


def validate_report(contract, report, expected_tree):
    require(set(report) == {"format", "edition", "phase", "task", "suite", "owner", "target",
            "backend", "profile", "probe_sha256", "source_tree_sha256", "git_revision", "toolchain",
            "flags", "host", "capture", "protocol", "strategy", "resource_model", "measurements"}, "report fields")
    require(report["format"] == "tondo-stdlib-cbor-performance-report/1", "report format")
    require(report["edition"] == contract["edition"] and report["phase"] == contract["phase"], "report edition")
    require(report["task"] == contract["task"] and report["owner"] == contract["owner"]
            and report["suite"] == "tondo-stdlib-cbor-performance", "report owner")
    require(all(report[field] == contract[field] for field in ("target", "backend", "profile",
            "protocol", "strategy", "resource_model")), "report boundary")
    require(report["probe_sha256"] == contract["probe"]["sha256"], "report probe")
    require(re.fullmatch(r"[0-9a-f]{64}", expected_tree)
            and report["source_tree_sha256"] == expected_tree, "source provenance drift")
    require(re.fullmatch(r"[0-9a-f]{40}", report["git_revision"]), "report revision")
    require(set(report["toolchain"]) == {"rustc", "cargo"}
            and all(type(value) is str for value in report["toolchain"].values())
            and report["toolchain"]["rustc"].startswith("rustc 1.93.0 ")
            and report["toolchain"]["cargo"].startswith("cargo 1.93.0 "), "pinned toolchain")
    require(set(report["flags"]) == {"rustflags", "encoded_rustflags", "incremental"}
            and all(type(value) is str for value in report["flags"].values()), "build flags")
    require(set(report["host"]) == {"cpu_model", "os"}
            and all(type(value) is str for value in report["host"].values()), "descriptive host fields")
    require(report["capture"] in {"development", "clean-source"}, "capture status")
    if report["capture"] == "clean-source":
        # A capture revision must contain the exact executable probe, including
        # after a later metadata-only closure commit advances main.
        result = subprocess.run(["git", "-C", str(ROOT), "show",
            f"{report['git_revision']}:{PROBE}"], capture_output=True, check=False)
        require(result.returncode == 0 and hashlib.sha256(result.stdout).hexdigest()
                == report["probe_sha256"], "capture revision lacks the exact probe")
    items = report["measurements"]
    require(len(items) == 15 and {item["workload_id"] for item in items} == set(ROUTES), "report workload set")
    rows = []
    for item in items:
        require(item["identity_sha256"] == digest(identity(report, item["workload_id"])), "report identity")
        for sample in item["samples"]:
            require(set(sample) == {"process", "repetition", "nanos"}, "retained sample fields")
            rows.append({"workload_id": item["workload_id"], "operation": item["operation"],
                "dispatch": item["dispatch"], "counters": item["counters"], **sample})
    expected = measurements(contract, rows)
    actual = [{key: value for key, value in item.items() if key != "identity_sha256"} for item in items]
    require(actual == expected, "report samples, percentiles, throughput or counters drift")
    return report


def render(contract, rows, context):
    report = {
        "format": "tondo-stdlib-cbor-performance-report/1", "edition": contract["edition"],
        "phase": contract["phase"], "task": contract["task"], "suite": "tondo-stdlib-cbor-performance",
        "owner": contract["owner"], "target": contract["target"], "backend": contract["backend"],
        "profile": contract["profile"], "probe_sha256": contract["probe"]["sha256"],
        "protocol": contract["protocol"], "strategy": contract["strategy"],
        "resource_model": contract["resource_model"], **context,
        "measurements": measurements(contract, rows),
    }
    for item in report["measurements"]:
        item["identity_sha256"] = digest(identity(report, item["workload_id"]))
    return validate_report(contract, report, context["source_tree_sha256"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("check-contract", "render", "check-report"))
    parser.add_argument("--contract", type=pathlib.Path, required=True)
    parser.add_argument("--samples", type=pathlib.Path)
    parser.add_argument("--report", type=pathlib.Path)
    parser.add_argument("--revision")
    parser.add_argument("--tree-sha256")
    parser.add_argument("--target")
    parser.add_argument("--rustc")
    parser.add_argument("--cargo")
    parser.add_argument("--rustflags", default="")
    parser.add_argument("--encoded-rustflags", default="")
    parser.add_argument("--incremental", default="")
    parser.add_argument("--cpu-model", default="unavailable")
    parser.add_argument("--os", default="unavailable")
    parser.add_argument("--capture", choices=("development", "clean-source"))
    args = parser.parse_args()
    contract = load_contract(args.contract)
    if args.mode == "check-contract":
        return
    require(args.report is not None and args.tree_sha256, "report and current source provenance required")
    if args.mode == "check-report":
        validate_report(contract, json.loads(args.report.read_text()), args.tree_sha256)
        return
    require(args.samples is not None and args.revision and args.target and args.rustc and args.cargo and args.capture,
            "capture inputs missing")
    require(args.target == contract["target"], "rustc host differs from pinned target")
    rows = [json.loads(line) for line in args.samples.read_text().splitlines()]
    report = render(contract, rows, {
        "git_revision": args.revision, "source_tree_sha256": args.tree_sha256,
        "toolchain": {"rustc": args.rustc, "cargo": args.cargo},
        "flags": {"rustflags": args.rustflags, "encoded_rustflags": args.encoded_rustflags,
                  "incremental": args.incremental},
        "host": {"cpu_model": args.cpu_model, "os": args.os}, "capture": args.capture,
    })
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError) as error:
        print(f"std.cbor performance: {error}", file=sys.stderr)
        sys.exit(1)
