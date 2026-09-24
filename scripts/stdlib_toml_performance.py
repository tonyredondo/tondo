#!/usr/bin/env python3
"""Validate and render the bounded std.toml scalar-kernel campaign."""

import argparse
import hashlib
import json
import math
import pathlib
import re
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parent.parent
IDS = {
    "parse-core-small", "parse-nested-medium", "parse-rows-large",
    "parse-temporal-rows", "parse-view-nested", "encode-nested",
    "encode-canonical", "reader-temporal-events", "writer-temporal-events",
    "reject-depth", "reject-nodes", "reject-scalar", "reject-duplicate-table",
}
OPERATIONS = {
    "materialized-parse", "borrowed-parse-view", "materialized-encode",
    "canonical-encode", "stream-reader-events", "stream-writer-events",
    "adversarial-reject",
}
COUNTERS = {
    "input_bytes", "output_bytes", "operations", "bytes_copied", "allocations",
    "logical_memory_bytes", "live_handles", "depth", "nodes", "tables",
    "array_table_rows", "event_count", "adversarial_rejections",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def canonical_bytes(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def digest(value):
    return hashlib.sha256(canonical_bytes(value)).hexdigest()


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b"\n") and not raw.endswith(b"\n\n"), "contract needs one final LF")
    require(b"\r" not in raw and all(line.rstrip() == line for line in raw.splitlines()),
            "contract has CR or trailing whitespace")
    contract = json.loads(raw)
    require(contract.get("format") == "tondo-stdlib-toml-performance/1", "invalid format")
    require(contract.get("edition") == "0.1" and contract.get("phase") == "STD-0.1B",
            "wrong edition or phase")
    require(contract.get("task") == "STD-TOML-PERF-001" and contract.get("owner") == "std.toml",
            "wrong owner")
    require(contract.get("status") == "verified-stdlib-kernel-baseline", "performance is not verified")
    require(contract.get("target") == "x86_64-unknown-linux-gnu", "target is not pinned")
    require(contract.get("backend") == "rust-stdlib-kernel" and contract.get("profile") == "test",
            "wrong backend or profile")
    probe = contract.get("probe", {})
    require(probe.get("path") == "crates/tondo-stdlib/tests/toml_performance.rs"
            and probe.get("test") == "toml_performance_probe", "wrong probe")
    require(re.fullmatch(r"[0-9a-f]{64}", probe.get("sha256", "")) is not None,
            "invalid probe hash")
    require((ROOT / probe["path"]).is_file(), "probe missing")
    require(hashlib.sha256((ROOT / probe["path"]).read_bytes()).hexdigest() == probe["sha256"],
            "probe hash mismatch")
    require(contract.get("protocol") == {
        "clock": "monotonic", "warmup_iterations": 3,
        "measurement_repetitions": 9, "independent_processes": 3,
        "minimum_sample_count": 27, "batch_operations": 16,
        "deterministic_seed": "tondo-stdlib-toml-perf-0.1",
        "outliers": "report-not-delete",
        "fixture_setup": "excluded-from-timed-latency; included-in-logical-allocation-and-memory-model",
    }, "protocol drift")
    require(set(contract.get("identity_fields", [])) == {
        "suite", "workload_id", "probe_sha256", "target", "backend", "profile",
        "toolchain", "flags", "git_revision",
    }, "identity fields drift")
    require(set(contract.get("forbidden_identity", [])) == {
        "ambient_environment", "cpu_frequency", "path", "pid", "timestamp",
    }, "forbidden identity fields drift")
    require(set(contract.get("metrics", [])) == {
        "latency", "tail_latency", "throughput", "bytes_copied", "allocations",
        "logical_memory_bytes", "depth", "nodes", "tables", "array_table_rows",
        "event_count", "adversarial_rejection", "live_handles", "dispatch",
    }, "missing metric")
    workloads = contract.get("workloads", [])
    require(len(workloads) == 13 and {item["id"] for item in workloads} == IDS,
            "workload set drift")
    for item in workloads:
        require(item["operation"] in OPERATIONS, "invalid operation")
        require(item["size_class"] in {"small", "medium", "large"}, "invalid size class")
        require(item["dispatch"] == "scalar-fixed-target", "dispatch drift")
        for field in ("payload_bytes", "depth", "nodes", "tables", "array_table_rows", "event_count"):
            require(type(item[field]) is int and 0 <= item[field] <= 16384,
                    f"invalid {field} in {item['id']}")
        require(0 < item["payload_bytes"] <= 4096, "fixture exceeds input bound")
        require(item["depth"] > 0, "fixture depth missing")
        if item["operation"] == "adversarial-reject":
            require(item["expected_error"] in {"DepthLimit", "NodeLimit", "ScalarLimit",
                                               "DuplicateTable"}, "rejection kind missing")
        else:
            require(item["expected_error"] is None, "successful workload has error")
    strategy = contract.get("strategy", {})
    require(strategy == {
        "stdlib_kernel": "direct-rust-scalar-kernel-baseline",
        "compiler_api": "not-claimed", "hosted_vm": "not-claimed-no-toml-bridge",
        "native_runtime_abi": "not-measured", "native_aot": "not-claimed",
        "simd": "not-measured-no-optimized-route", "multiversion_dispatch": "not-claimed",
        "selection": "scalar-only",
    }, "promotion boundary drift")
    require(contract.get("report") == "target/reliability/evidence/stdlib-toml-performance.json",
            "report path drift")
    return contract


def percentile(samples, fraction):
    return samples[math.ceil(len(samples) * fraction) - 1]


def make_measurements(contract, rows):
    grouped = defaultdict(list)
    for row in rows:
        require(set(row) == {"workload_id", "operation", "size_class", "nanos", "dispatch",
                             "counters"}, "sample field drift")
        require(row["workload_id"] in IDS, "unknown workload")
        require(type(row["nanos"]) is int and row["nanos"] > 0, "invalid monotonic sample")
        require(set(row["counters"]) == COUNTERS, "counter field drift")
        grouped[row["workload_id"]].append(row)
    require(set(grouped) == IDS, "missing workload")
    measurements = []
    for spec in sorted(contract["workloads"], key=lambda item: item["id"]):
        samples = grouped[spec["id"]]
        require(len(samples) == 27, f"{spec['id']} needs 27 retained samples")
        first = samples[0]
        for sample in samples:
            require({key: value for key, value in sample.items() if key != "nanos"} ==
                    {key: value for key, value in first.items() if key != "nanos"},
                    f"{spec['id']} counters or dispatch changed between samples")
        counters = first["counters"]
        require(first["operation"] == spec["operation"] and
                first["size_class"] == spec["size_class"] and
                first["dispatch"] == spec["dispatch"], "sample route drift")
        require(counters["input_bytes"] == spec["payload_bytes"] and
                counters["operations"] == 16, "payload or batch drift")
        for field in ("depth", "nodes", "tables", "array_table_rows", "event_count"):
            require(counters[field] == spec[field], f"{spec['id']} {field} drift")
        require(all(type(value) is int and value >= 0 for value in counters.values()),
                "negative or noninteger counter")
        require(counters["allocations"] > 0 and counters["logical_memory_bytes"] > 0,
                "missing logical resource counters")
        require(counters["live_handles"] == 0, "reader or writer handle remained live")
        rejecting = spec["operation"] == "adversarial-reject"
        require(counters["adversarial_rejections"] == (16 if rejecting else 0),
                "rejection count drift")
        require(rejecting or counters["bytes_copied"] > 0, "copy counter missing")
        require((spec["operation"] in {"materialized-encode", "canonical-encode",
                                        "stream-writer-events"}) == (counters["output_bytes"] > 0),
                "output byte count drift")
        elapsed = sorted(sample["nanos"] for sample in samples)
        median = percentile(elapsed, 0.50)
        p95 = percentile(elapsed, 0.95)
        p99 = percentile(elapsed, 0.99)
        throughput_bytes = (counters["output_bytes"] if spec["operation"] in {
            "materialized-encode", "canonical-encode", "stream-writer-events"
        } else counters["input_bytes"]) * 16
        measurements.append({
            "workload_id": spec["id"], "operation": spec["operation"],
            "size_class": spec["size_class"], "payload_bytes": spec["payload_bytes"],
            "expected_error": spec["expected_error"], "dispatch": spec["dispatch"],
            "sample_count": len(elapsed), "samples_ns": elapsed,
            "median_ns": median, "p95_ns": p95, "p99_ns": p99,
            "throughput_bytes_per_second": throughput_bytes * 1_000_000_000 // median,
            "counters": counters,
        })
    return measurements


def identity(report, workload_id):
    return {
        "suite": report["suite"], "workload_id": workload_id,
        "probe_sha256": report["probe_sha256"], "target": report["target"],
        "backend": report["backend"], "profile": report["profile"],
        "toolchain": report["toolchain"], "flags": report["flags"],
        "git_revision": report["git_revision"],
    }


def validate_report(contract, report):
    require(report.get("format") == "tondo-stdlib-toml-performance-report/1", "report format")
    require(report.get("task") == contract["task"] and report.get("owner") == contract["owner"],
            "report owner")
    require(report.get("target") == contract["target"] and
            report.get("backend") == contract["backend"] and
            report.get("profile") == contract["profile"], "report target")
    require(report.get("probe_sha256") == contract["probe"]["sha256"], "report probe hash")
    require(report.get("protocol") == contract["protocol"] and
            report.get("strategy") == contract["strategy"], "report protocol or strategy")
    require(re.fullmatch(r"[0-9a-f]{40}", report.get("git_revision", "")) is not None,
            "report revision")
    measurements = report.get("measurements", [])
    require(len(measurements) == 13 and
            {item["workload_id"] for item in measurements} == IDS, "report workload set")
    specs = {item["id"]: item for item in contract["workloads"]}
    rows = []
    for item in measurements:
        spec = specs[item["workload_id"]]
        samples = item["samples_ns"]
        require(len(samples) == item["sample_count"] == 27 and samples == sorted(samples),
                "report samples missing or unsorted")
        require(all(type(sample) is int and sample > 0 for sample in samples), "report sample")
        require(item["median_ns"] == percentile(samples, 0.50) and
                item["p95_ns"] == percentile(samples, 0.95) and
                item["p99_ns"] == percentile(samples, 0.99), "report percentile")
        require(item["payload_bytes"] == spec["payload_bytes"] and
                item["operation"] == spec["operation"] and
                item["expected_error"] == spec["expected_error"], "report fixture")
        require(item["dispatch"] == "scalar-fixed-target" and
                item["counters"]["live_handles"] == 0, "report dispatch or lifecycle")
        require(item["identity_sha256"] == digest(identity(report, item["workload_id"])),
                "report identity")
        rows.extend({"workload_id": item["workload_id"], "operation": item["operation"],
                     "size_class": item["size_class"], "nanos": sample,
                     "dispatch": item["dispatch"], "counters": item["counters"]}
                    for sample in samples)
    expected = make_measurements(contract, rows)
    require([{key: value for key, value in item.items() if key != "identity_sha256"}
             for item in measurements] == expected, "report dimensions or counters drift")
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["check-contract", "render", "check-report"])
    parser.add_argument("--contract", type=pathlib.Path, required=True)
    parser.add_argument("--samples", type=pathlib.Path)
    parser.add_argument("--report", type=pathlib.Path)
    parser.add_argument("--revision")
    parser.add_argument("--target")
    parser.add_argument("--rustc")
    parser.add_argument("--cargo")
    parser.add_argument("--flags", default="")
    parser.add_argument("--cpu-model", default="unavailable")
    parser.add_argument("--os", default="unavailable")
    args = parser.parse_args()
    contract = load_contract(args.contract)
    if args.mode == "check-contract":
        return
    require(args.report is not None, "report path required")
    if args.mode == "check-report":
        validate_report(contract, json.loads(args.report.read_text()))
        return
    require(args.samples is not None and args.revision and args.target and args.rustc and
            args.cargo, "render metadata missing")
    require(args.target == contract["target"], "runtime target differs from pinned target")
    rows = [json.loads(line) for line in args.samples.read_text().splitlines()]
    report = {
        "format": "tondo-stdlib-toml-performance-report/1", "edition": "0.1",
        "phase": "STD-0.1B", "task": contract["task"],
        "suite": "tondo-stdlib-toml-performance", "owner": contract["owner"],
        "target": args.target, "backend": contract["backend"],
        "profile": contract["profile"], "probe_sha256": contract["probe"]["sha256"],
        "git_revision": args.revision,
        "toolchain": {"rustc": args.rustc, "cargo": args.cargo},
        "flags": args.flags,
        "host": {"cpu_model": args.cpu_model, "os": args.os},
        "protocol": contract["protocol"], "strategy": contract["strategy"],
        "measurements": make_measurements(contract, rows),
    }
    for item in report["measurements"]:
        item["identity_sha256"] = digest(identity(report, item["workload_id"]))
    validate_report(contract, report)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (KeyError, TypeError, ValueError, OSError, json.JSONDecodeError) as error:
        print(f"std.toml performance: {error}", file=sys.stderr)
        sys.exit(1)
