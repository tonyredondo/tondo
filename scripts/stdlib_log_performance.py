"""Strict logging performance contracts and retained hosted VM reports."""

from collections import defaultdict
import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
PROBE = "crates/tondo-compiler/src/process_host/log/performance.rs"
PROBE_TEST = "process_host::log::performance::logging_performance_probe"

PROCESSES = 3
REPETITIONS = 9
SAMPLES_PER_ROUTE = PROCESSES * REPETITIONS
MAX_SAMPLE_NS = 30_000_000_000
MAX_COUNTER = 2_147_483_648

PROTOCOL = {
    "clock": "std::time::Instant-monotonic", "warmup_repetitions": 3,
    "measured_repetitions_per_process": 9, "independent_processes": 3,
    "minimum_sample_count": 27, "outliers": "retain-all",
    "percentiles": "nearest-rank", "sample_order": "process-then-repetition",
    "timed": "whole-execute_with_limits-including-VM-verification-setup-event-construction-delivery-close-and-Tondo-assertions",
    "excluded_latency": ["compiler", "oracle", "fixture-preload", "Rust-byte-and-terminal-checks"],
    "independent_process_timeout_seconds": 120,
}
STRATEGY = {
    "selected": "hosted-scalar", "hosted_vm": "measured-whole-workload",
    "public_logger": "fifteen-routes", "sealed_buffered_writer": "three-test-only-routes",
    "native_abi": "unmeasured", "native_aot": "unmeasured", "SIMD": "not-claimed",
    "multiversion_dispatch": "not-claimed", "code_size": "unmeasured",
    "production_latency": "not-claimed-test-profile", "speedup": "not-claimed-no-comparison",
}
RESOURCE_MODEL = {
    "vm_allocations": "observed-VmStatistics-logical-managed-object-allocations-not-allocator-calls",
    "vm_peak_live_bytes": "observed-VmStatistics-logical-managed-heap-peak-not-full-host-memory-or-RSS",
    "collection_copies": "observed-VmStatistics-logical-buffer-copies-elements-shares-and-detaches",
    "host_handles_created": "actual-whole-workload-host-registry-ID-increment",
    "queue_high_water": "independent-model-replay-not-sampled-host-peak",
    "selected_fixture_bytes": "retained-source-sealed-source-authored-records-expected-output-and-file-prefix",
    "selected_fixture_allocations": "retained-selected-String-and-Vec-identities-not-allocator-calls",
    "terminal": "actual-host-registries-channels-jobs-waiters-after-consuming-close",
    "budget_bytes": "unmeasured-null-ordinary-logging-has-no-shared-provider-budget",
    "native_live_handles": "unmeasured-null",
    "excluded": ["compiler-and-bytecode-storage", "oracle-temporaries", "stack-and-capacity-slack",
                 "host-map-and-Arc-nodes", "writer-provider-internals", "OS-allocator", "native-runtime"],
}
ORACLE = {
    "kind": "independent-bounded-values-queue-and-authored-large-ASCII-law",
    "sources": ["crates/tondo-reliability/src/log_model/values.rs",
                "crates/tondo-reliability/src/log_model/queue.rs", PROBE,
                "crates/tondo-compiler/tests/fixtures/log-writer-probe.to"],
    "exact": ["formatted-bytes", "receipts", "nominal-errors", "retained-prefix", "consuming-close"],
    "concurrency": "whole-records-each-once-and-per-producer-order-replayed-against-legal-observed-order",
    "large_message": "4096-ASCII-x-outside-values-domain-authored-JSON-byte-law-no-domain-promotion",
    "floats": "shared-Rust-primitive-no-independent-numeric-rendering-claim",
    "timestamps": "supplied-authored-UTC-token-no-calendar-or-clock-provider-oracle",
}
FORBIDDEN_IDENTITY = ["ambient_environment", "cpu_frequency", "path", "pid", "timestamp"]
LIMITS = {"sample_latency_ns": MAX_SAMPLE_NS, "selected_counter": MAX_COUNTER,
          "vm_steps": 2_000_000, "vm_heap_bytes": 8_388_608,
          "capture_samples": 486, "capture_report_bytes": 33_554_432}


def route(name, operation, count, format_name="Text", capacity=8, flow="normal", shape="plain"):
    accepted = count
    filtered = dropped = rejected = 0
    attempted = count
    peak = min(count, capacity)
    if flow == "query":
        attempted = accepted = peak = 0
    elif flow == "filtered":
        filtered = count
        accepted = peak = 0
    elif flow in {"reject", "drop"}:
        accepted = peak = 1
        rejected = count - 1 if flow == "reject" else 0
        dropped = count - 1 if flow == "drop" else 0
    elif flow == "limit":
        rejected = count
        accepted = peak = 0
    elif flow == "io":
        attempted += 1
        rejected = 1
    elif flow == "flush":
        peak = 1
    expected = {"operations": count if flow == "query" else attempted,
                "attempted_events": attempted, "accepted_events": accepted,
                "filtered_events": filtered, "dropped_events": dropped,
                "rejected_events": rejected, "delivered_records": 0 if flow == "io" else accepted,
                "model_peak_queued_records": peak}
    if not accepted:
        expected.update(output_bytes=0, model_peak_queued_bytes=0)
    elif flow == "io":
        expected["output_bytes"] = 2
    return {"id": name, "operation": operation, "format": format_name,
            "capacity": capacity, "shape": shape,
            "api": "sealed-buffered-writer" if flow in {"short", "interrupt", "io"} else "public-Logger",
            "expected_counters": expected}


ROUTES = {value["id"]: value for value in (
    route("enabled-query", "enabled-query", 128, flow="query"),
    route("filtered-emit", "filtered-emit", 128, flow="filtered"),
    route("text-small", "emit", 32), route("json-small", "emit", 32, "JsonLines"),
    route("text-structured", "emit", 16, shape="structured"),
    route("json-structured", "emit", 16, "JsonLines", shape="structured"),
    route("json-large", "emit", 8, "JsonLines", shape="large"),
    route("block-capacity-one", "block", 32, "JsonLines", capacity=1),
    route("reject-full", "reject", 8, capacity=1, flow="reject"),
    route("drop-full", "drop", 8, capacity=1, flow="drop"),
    route("flush-each", "flush", 8, flow="flush"),
    route("concurrent-producers", "concurrent", 32, shape="four-producers"),
    route("file-append", "file", 8, "JsonLines"), route("file-truncate", "file", 8, "JsonLines"),
    route("writer-short", "short-write", 8, flow="short"),
    route("writer-interrupt", "interruption-resume", 8, flow="interrupt"),
    route("writer-io", "terminal-io", 2, capacity=2, flow="io"),
    route("reject-event-limit", "limit-rejection", 8, flow="limit"),
)}
IDENTITY_FIELDS = {
    "suite", "workload_id", "probe_sha256", "source_tree_sha256", "target",
    "backend", "profile", "toolchain", "flags", "git_revision",
}
COUNTERS = {
    "operations", "attempted_events", "accepted_events", "filtered_events",
    "dropped_events", "rejected_events", "delivered_records", "output_bytes", "vm_steps",
    "vm_allocations", "vm_collections", "vm_peak_live_objects",
    "vm_peak_live_bytes", "logical_collection_copies", "collection_elements_copied",
    "collection_buffer_shares", "collection_buffer_detaches", "host_handles_created",
    "model_peak_queued_records", "model_peak_queued_bytes",
    "selected_fixture_bytes", "selected_fixture_allocations",
}
TERMINAL_FIELDS = {"host_handles", "channels", "jobs", "waiters", "budget_bytes"}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON key: {key}")
        result[key] = value
    return result


def strict_json(raw):
    def invalid_constant(value):
        raise ValueError(f"non-finite JSON number: {value}")
    def finite_float(raw_number):
        value = float(raw_number)
        require(math.isfinite(value), "non-finite JSON exponent")
        return value
    return json.loads(raw, object_pairs_hook=unique_object, parse_constant=invalid_constant,
                      parse_float=finite_float)


def digest(value):
    raw = json.dumps(value, sort_keys=True, separators=(",", ":"),
                     ensure_ascii=False, allow_nan=False).encode()
    return hashlib.sha256(raw).hexdigest()


def natural(value, maximum, name):
    require(type(value) is int and 0 <= value <= maximum, f"{name}: bounded integer")


def nearest_rank(values, fraction):
    require(values and 0 < fraction <= 1, "percentile domain")
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def distribution(values):
    require(values, "empty distribution")
    for value in values:
        natural(value, MAX_COUNTER, "counter")
    return {
        "min": min(values), "median": nearest_rank(values, .5),
        "p95": nearest_rank(values, .95), "p99": nearest_rank(values, .99),
        "max": max(values),
    }


def validate_identity(identity):
    require(type(identity) is dict and set(identity) == IDENTITY_FIELDS, "identity fields")
    require(identity["suite"] == "stdlib-log" and identity["target"] == "x86_64-unknown-linux-gnu"
            and identity["backend"] == "hosted-bytecode-vm" and identity["profile"] == "test",
            "qualified route")
    require(type(identity["workload_id"]) is str
            and re.fullmatch(r"[a-z][a-z0-9-]{0,63}", identity["workload_id"]), "workload identity")
    for field in ("probe_sha256", "source_tree_sha256"):
        require(type(identity[field]) is str
                and re.fullmatch(r"[0-9a-f]{64}", identity[field]), f"{field}: hash")
    require(type(identity["git_revision"]) is str
            and re.fullmatch(r"[0-9a-f]{40}", identity["git_revision"]), "Git revision")
    require(type(identity["toolchain"]) is dict
            and set(identity["toolchain"]) == {"rustc", "cargo"}
            and all(type(value) is str and value for value in identity["toolchain"].values()),
            "toolchain identity")
    require(type(identity["flags"]) is dict
            and set(identity["flags"]) == {"rustflags", "encoded_rustflags", "incremental"}
            and all(type(value) is str for value in identity["flags"].values()), "flags identity")


def validate_sample(sample, routes):
    require(type(sample) is dict and set(sample) == {
        "workload_id", "operation", "process", "repetition", "nanos",
        "dispatch", "counters", "terminal", "native_live_handles",
    }, "sample fields")
    name = sample["workload_id"]
    require(type(name) is str and name in routes, "unknown workload")
    route = routes[name]
    require(sample["operation"] == route["operation"]
            and sample["dispatch"] == "hosted-scalar", "sample route")
    natural(sample["process"], PROCESSES, "process")
    require(sample["process"] >= 1, "process starts at one")
    natural(sample["repetition"], REPETITIONS - 1, "repetition")
    natural(sample["nanos"], MAX_SAMPLE_NS, "latency")
    require(sample["nanos"] > 0, "zero latency")
    counters = sample["counters"]
    require(type(counters) is dict and set(counters) == COUNTERS, "counter fields")
    for field, value in counters.items():
        natural(value, MAX_COUNTER, field)
    for field, value in route["expected_counters"].items():
        require(field in COUNTERS and counters[field] == value, f"{field}: oracle count")
    require(counters["operations"] > 0 and counters["vm_steps"] > 0
            and counters["vm_allocations"] > 0 and counters["vm_peak_live_bytes"] > 0,
            "missing VM execution")
    require(counters["attempted_events"] == sum(counters[field] for field in (
        "accepted_events", "filtered_events", "dropped_events", "rejected_events")),
        "receipt conservation")
    require(counters["delivered_records"] <= counters["accepted_events"], "delivery conservation")
    if "capacity" in route:
        require(counters["model_peak_queued_records"] <= route["capacity"], "queue capacity")
        require(counters["vm_steps"] <= LIMITS["vm_steps"]
                and counters["vm_peak_live_bytes"] <= LIMITS["vm_heap_bytes"], "VM resource limits")
        require(counters["selected_fixture_bytes"] > 0
                and counters["selected_fixture_allocations"] > 0, "unrepresented fixture setup")
        require(counters["output_bytes"] > 0 or counters["delivered_records"] == 0,
                "delivered record without bytes")
    require(counters["model_peak_queued_records"] <= counters["accepted_events"]
            and (counters["model_peak_queued_bytes"] == 0)
            == (counters["model_peak_queued_records"] == 0), "queue model conservation")
    require(type(sample["terminal"]) is dict and set(sample["terminal"]) == TERMINAL_FIELDS,
            "terminal fields")
    require(all(type(value) is int and value == 0 for field, value in sample["terminal"].items()
                if field != "budget_bytes"),
            "terminal resource leak")
    require(sample["terminal"]["budget_bytes"] is None, "ordinary host budget is unmeasured")
    require(sample["native_live_handles"] is None, "native route is unmeasured")


def render_workloads(rows, routes):
    require(type(rows) is list and len(rows) == len(routes) * SAMPLES_PER_ROUTE, "sample count")
    grouped = defaultdict(list)
    for row in rows:
        validate_sample(row, routes)
        grouped[row["workload_id"]].append(row)
    require(set(grouped) == set(routes), "incomplete workloads")
    expected_coordinates = {(p, r) for p in range(1, PROCESSES + 1) for r in range(REPETITIONS)}
    workloads = []
    for name in sorted(routes):
        samples = grouped[name]
        require(len(samples) == SAMPLES_PER_ROUTE
                and {(row["process"], row["repetition"]) for row in samples} == expected_coordinates,
                "missing or duplicate sample coordinate")
        nanos = [row["nanos"] for row in samples]
        median = nearest_rank(nanos, .5)
        operation_counts = {row["counters"]["operations"] for row in samples}
        require(len(operation_counts) == 1, "operation denominator drift")
        accepted_counts = {row["counters"]["accepted_events"] for row in samples}
        require(len(accepted_counts) == 1, "accepted denominator drift")
        delivered_counts = {row["counters"]["delivered_records"] for row in samples}
        require(len(delivered_counts) == 1, "delivered denominator drift")
        workloads.append({
            "workload_id": name, "operation": routes[name]["operation"],
            "dispatch": "hosted-scalar", "sample_count": SAMPLES_PER_ROUTE,
            "latency_ns": {"median": median, "p95": nearest_rank(nanos, .95),
                           "p99": nearest_rank(nanos, .99), "max": max(nanos)},
            "operations_per_second": next(iter(operation_counts)) * 1_000_000_000 / median,
            "accepted_events_per_second": next(iter(accepted_counts)) * 1_000_000_000 / median,
            "delivered_records_per_second": next(iter(delivered_counts)) * 1_000_000_000 / median,
            "counter_distributions": {field: distribution([row["counters"][field] for row in samples])
                                      for field in sorted(COUNTERS)},
            "samples": sorted(samples, key=lambda row: (row["process"], row["repetition"])),
        })
    return workloads


def verify_workloads(workloads, routes):
    require(type(workloads) is list and len(workloads) == len(routes), "workload count")
    rows = []
    for workload in workloads:
        require(type(workload) is dict and set(workload) == {
            "workload_id", "operation", "dispatch", "sample_count", "latency_ns",
            "operations_per_second", "accepted_events_per_second", "delivered_records_per_second",
            "counter_distributions", "samples",
        }, "workload fields")
        require(type(workload["samples"]) is list, "retained samples")
        rows.extend(workload["samples"])
    require(digest(workloads) == digest(render_workloads(rows, routes)), "derived summary mismatch")


def contract_value(status):
    require(status in {"measurement-ready", "verified-hosted-scalar-baseline"}, "contract status")
    return {
        "format": "tondo-stdlib-log-performance/1", "edition": "0.1", "phase": "STD-0.1B",
        "task": "STD-LOG-PERF-001", "owner": "std.log", "status": status,
        "target": "x86_64-unknown-linux-gnu", "backend": "hosted-bytecode-vm", "profile": "test",
        "contract": "docs/contracts/stdlib-log-performance.md", "parent_contract": "testing/stdlib-log.json",
        "probe": {"path": PROBE, "test": PROBE_TEST,
                  "sha256": hashlib.sha256((ROOT / PROBE).read_bytes()).hexdigest()},
        "protocol": PROTOCOL, "strategy": STRATEGY, "resource_model": RESOURCE_MODEL,
        "limits": LIMITS, "oracle": ORACLE, "identity_fields": sorted(IDENTITY_FIELDS),
        "forbidden_identity": FORBIDDEN_IDENTITY, "workloads": list(ROUTES.values()),
        "metrics": sorted(COUNTERS | {"latency_ns", "operations_per_second",
                                      "accepted_events_per_second", "delivered_records_per_second",
                                      "terminal", "native_live_handles"}),
        "report": {"format": "tondo-stdlib-log-performance-report/1",
                   "path": "target/reliability/evidence/stdlib-log-performance.json",
                   "portable_artifact": False},
    }


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b"\n") and not raw.endswith(b"\n\n") and b"\r" not in raw
            and all(line.rstrip() == line for line in raw.splitlines()), "canonical whitespace")
    contract = strict_json(raw)
    require(type(contract) is dict and "status" in contract, "contract object")
    require(digest(contract) == digest(contract_value(contract["status"])),
            "contract, workload or probe boundary drift")
    return contract


def validate_report(contract, value, tree):
    require(type(value) is dict and set(value) == {
        "format", "edition", "phase", "task", "owner", "capture", "context", "host",
        "protocol", "strategy", "resource_model", "workloads",
    }, "report fields")
    require(value["format"] == contract["report"]["format"], "report format")
    for field in ("edition", "phase", "task", "owner", "protocol", "strategy", "resource_model"):
        require(digest(value[field]) == digest(contract[field]), f"report {field} boundary")
    context = value["context"]
    require(type(context) is dict and set(context) == IDENTITY_FIELDS - {"workload_id"}, "context fields")
    validate_identity({**context, "workload_id": "enabled-query"})
    require(context["source_tree_sha256"] == tree
            and context["probe_sha256"] == contract["probe"]["sha256"], "report source binding")
    require(value["capture"] in {"development", "clean-source"}, "capture mode")
    require(type(value["host"]) is dict and set(value["host"]) == {"cpu_model", "os"}
            and all(type(item) is str and item for item in value["host"].values()), "host description")
    require(type(value["workloads"]) is list, "workload list")
    plain = []
    for workload in value["workloads"]:
        require(type(workload) is dict and {"identity", "identity_sha256"} <= workload.keys(),
                "workload identity")
        identity = {**context, "workload_id": workload.get("workload_id")}
        validate_identity(identity)
        require(digest(workload["identity"]) == digest(identity)
                and workload["identity_sha256"] == digest(identity), "workload provenance")
        plain.append({field: item for field, item in workload.items()
                      if field not in {"identity", "identity_sha256"}})
    verify_workloads(plain, ROUTES)
    if value["capture"] == "clean-source":
        committed = subprocess.run(["git", "-C", str(ROOT), "show",
                                    f"{context['git_revision']}:{PROBE}"], capture_output=True, check=False)
        require(committed.returncode == 0
                and hashlib.sha256(committed.stdout).hexdigest() == context["probe_sha256"],
                "capture revision lacks the exact probe")
    return value


def render(contract, rows, context, host, capture):
    workloads = render_workloads(rows, ROUTES)
    for workload in workloads:
        identity = {**context, "workload_id": workload["workload_id"]}
        workload.update(identity=identity, identity_sha256=digest(identity))
    value = {
        "format": contract["report"]["format"],
        **{field: contract[field] for field in ("edition", "phase", "task", "owner", "protocol",
                                              "strategy", "resource_model")},
        "capture": capture, "context": context, "host": host, "workloads": workloads,
    }
    return validate_report(contract, value, context["source_tree_sha256"])


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("write-contract", "check-contract", "render", "check-report"))
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--status", default="measurement-ready")
    parser.add_argument("--samples", type=Path)
    parser.add_argument("--report", type=Path)
    for field in ("revision", "tree-sha256", "target", "rustc", "cargo", "capture"):
        parser.add_argument("--" + field)
    for field in ("rustflags", "encoded-rustflags", "incremental", "cpu-model", "os"):
        parser.add_argument("--" + field, default="")
    args = parser.parse_args()
    if args.mode == "write-contract":
        write_json(args.contract, contract_value(args.status))
        return
    contract = load_contract(args.contract)
    if args.mode == "check-contract":
        return
    require(args.report is not None and args.tree_sha256, "report and source inputs required")
    if args.mode == "check-report":
        require(args.report.stat().st_size <= LIMITS["capture_report_bytes"], "report size limit")
        validate_report(contract, strict_json(args.report.read_bytes()), args.tree_sha256)
        return
    require(args.samples is not None and args.revision and args.rustc and args.cargo and args.capture
            and args.target == contract["target"], "capture inputs")
    require(args.samples.stat().st_size <= LIMITS["capture_report_bytes"], "sample file size limit")
    context = {
        "suite": "stdlib-log", "probe_sha256": contract["probe"]["sha256"],
        "source_tree_sha256": args.tree_sha256, "git_revision": args.revision,
        "target": args.target, "backend": contract["backend"], "profile": contract["profile"],
        "toolchain": {"rustc": args.rustc, "cargo": args.cargo},
        "flags": {"rustflags": args.rustflags, "encoded_rustflags": args.encoded_rustflags,
                  "incremental": args.incremental},
    }
    rows = [strict_json(line) for line in args.samples.read_text().splitlines()]
    value = render(contract, rows, context, {"cpu_model": args.cpu_model, "os": args.os}, args.capture)
    write_json(args.report, value)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, AttributeError, OSError) as error:
        print(f"std.log performance: {error}", file=sys.stderr)
        sys.exit(1)
