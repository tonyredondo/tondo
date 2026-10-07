"""Validate target-qualified hosted networking samples without discarding variation."""

import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import sys


ROUTES = {
    "tcp-connect-accept": ("tcp-lifecycle", 0, 0),
    "tcp-read-small": ("tcp-read", 64, 0),
    "tcp-read-chunked": ("tcp-read", 65536, 0),
    "tcp-read-rollback": ("tcp-read", 64, 0),
    "tcp-write-small": ("tcp-write", 64, 0),
    "tcp-write-large": ("tcp-write", 65536, 0),
    "tcp-write-backpressure": ("tcp-write", 8388608, 0),
    "udp-send-empty": ("udp-send", 0, 0),
    "udp-send-small": ("udp-send", 64, 0),
    "udp-receive-empty": ("udp-receive", 0, 0),
    "udp-receive-small": ("udp-receive", 64, 0),
    "udp-reject-oversized": ("udp-receive", 64, 1),
    "dns-ordered": ("dns", 0, 0),
    "dns-delay": ("dns", 0, 0),
    "dns-result-limit": ("dns", 0, 1),
    "tls12-echo": ("tls", 64, 0),
    "tls13-echo": ("tls", 64, 0),
    "tls-reject-name": ("tls", 0, 1),
    "pending-accept-cancel": ("cancellation", 0, 1),
    "reject-expired-deadline": ("admission", 0, 1),
    "reject-host-memory": ("admission", 0, 1),
}
COUNTERS = {
    "host_calls", "polls", "partial_replies", "pending_polls", "rollbacks",
    "cancellations", "rejections", "dns_queries", "selected_allocations",
    "bytes_copied", "sampled_budget_peak_bytes", "reply_retained_peak_bytes",
    "host_handles_created", "selected_fixture_bytes", "selected_fixture_allocations",
    "backpressure_pending_polls",
}
TERMINAL = {"live_handles", "live_jobs", "pending_slots", "budget_bytes"}
IDENTITY = {
    "suite", "workload_id", "probe_sha256", "source_tree_sha256", "target",
    "backend", "profile", "toolchain", "flags", "git_revision",
}
CONTEXT = IDENTITY - {"workload_id"}
ROOT = Path(__file__).resolve().parent.parent
PROBE = "crates/tondo-compiler/src/process_host/net/performance.rs"
ERRORS = {
    "udp-reject-oversized": "NetError.DatagramTooLarge",
    "dns-result-limit": "NetError.ResourceLimit",
    "tls-reject-name": "TlsError.CertificateRejected",
    "pending-accept-cancel": "NetError.Cancelled",
    "reject-expired-deadline": "NetError.Timeout",
    "reject-host-memory": "VmMemoryResourceLimit",
}
PROTOCOL = {
    "clock": "monotonic", "warmup_iterations": 3, "measurement_repetitions": 9,
    "independent_processes": 3, "minimum_sample_count": 27,
    "logical_workloads_per_sample": 1, "outliers": "report-not-delete",
    "sample_coordinates": "process-1-through-3-and-repetition-0-through-8",
    "fixture_setup": "excluded-from-latency; selected-resource-observations-retained",
    "timed_interval": "host-admission-polling-return-checks-result-disposal-and-explicit-close",
    "post_timing": "peer-byte-checks-join-private-root-collection-zero-terminal-and-reactor-drop",
}
STRATEGY = {
    "selected_route": "hosted-scalar", "compiler_api": "inherited-verified-production-hosted",
    "hosted_bridge": "direct-private-bootstrap-host-with-reply-admission-and-owner-polling",
    "hosted_vm": "not-measured-no-bytecode-execution", "native_runtime_abi": "not-measured",
    "native_aot": "not-measured", "simd": "not-measured-no-tondo-optimized-route",
    "multiversion_dispatch": "not-claimed", "code_size": "not-measured",
    "providers": "controlled-loopback-explicit-DNS-TLS12-TLS13-and-pinned-adapters",
    "dependency_crypto_acceleration": "not-a-tondo-simd-promotion",
    "peer_end_to_end_throughput": "not-measured-host-interval-only",
}
RESOURCE_MODEL = {
    "scope": "selected-setup-timed-and-teardown-observations-per-complete-workload",
    "copies": "timed-delivered-read-bytes-UDP-accessor-and-argument-provider-input-copies; excludes-fixture-construction",
    "allocations": "selected-result-box-container-name-provider-future-job-and-owned-buffer-identities; not-allocator-calls",
    "fixture_storage": "explicitly-retained-authored-byte-vectors-and-certificate-DER",
    "logical_memory": "retained-selected-fixture-bytes-plus-sampled-shared-admission-charge; estimate-not-RSS-or-full-peak",
    "reply_retained_bytes": "observed-maximum-typed-return-charge-before-result-disposal",
    "host_handles_created": "actual-whole-workload-registry-ID-increment-including-setup",
    "terminal": "actual-registry-network-jobs-executor-slots-and-shared-charge-after-private-cleanup",
    "memory_refusal": "held-logical-admission-charge-does-not-allocate-equivalent-OS-bytes",
    "native_live_handles": "unmeasured-null",
    "excluded": ["oracle-temporaries", "stack-buffers", "capacity-slack", "host-map-nodes",
                 "Arc-control-blocks", "clock-carrier-metadata", "provider-and-crypto-internals",
                 "private-reactor", "OS-allocator-and-native-runtime"],
}
LIMITS = {"fixture_bytes": 8388608, "reference_bytes": 128, "reference_addresses": 32,
          "sample_latency_ns": 30000000000, "selected_counter": 2147483648,
          "provider_wait_seconds": 5}
ORACLE = {
    "kind": "independent-finite-admission-stream-TLS-verdict-and-authored-peer-laws",
    "sources": ["crates/tondo-reliability/src/net_model.rs",
                "crates/tondo-reliability/src/net_model/admission.rs",
                "crates/tondo-reliability/src/net_model/stream.rs",
                "crates/tondo-reliability/src/net_model/tls.rs", PROBE],
    "large_payload": "bytes(index % 251); peer-checks-every-byte; OutsideDomain-not-a-target-error",
    "DNS": "two-explicit-A-then-AAAA-requests; first-seen-deduplication; whole-result-limit",
    "TLS": "model-verdict-ownership-plus-real-existing-local-certificate-fixtures; no-independent-record-parser",
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                    ensure_ascii=False, allow_nan=False).encode()).hexdigest()


def nearest_rank(values, fraction):
    ordered = sorted(values)
    require(bool(ordered), "empty observations")
    return ordered[max(0, math.ceil(len(ordered) * fraction) - 1)]


def summary(values):
    require(all(type(value) is int and value >= 0 for value in values), "observation type")
    return {
        "min": min(values), "median": nearest_rank(values, 0.5),
        "p95": nearest_rank(values, 0.95), "p99": nearest_rank(values, 0.99),
        "max": max(values),
    }


def validate_context(context):
    require(set(context) == CONTEXT, "identity context fields")
    require(all(type(context[key]) is str for key in CONTEXT - {"toolchain", "flags"}),
            "identity context types")
    require(context["suite"] == "tondo-stdlib-net-performance", "suite identity")
    require(context["target"] == "x86_64-unknown-linux-gnu"
            and context["backend"] == "rust-hosted-bridge"
            and context["profile"] == "test", "measurement route")
    require(re.fullmatch(r"[0-9a-f]{64}", context["probe_sha256"])
            and re.fullmatch(r"[0-9a-f]{64}", context["source_tree_sha256"])
            and re.fullmatch(r"[0-9a-f]{40}", context["git_revision"]), "source identity")
    require(set(context["toolchain"]) == {"rustc", "cargo"}
            and all(type(value) is str for value in context["toolchain"].values())
            and context["toolchain"]["rustc"].startswith("rustc 1.93.0 ")
            and context["toolchain"]["cargo"].startswith("cargo 1.93.0 "), "toolchain identity")
    require(set(context["flags"]) == {"rustflags", "encoded_rustflags", "incremental"}
            and all(type(value) is str for value in context["flags"].values()), "flag identity")


def validate_sample(name, row):
    require(name in ROUTES, "unknown workload")
    require(set(row) == {"workload_id", "operation", "dispatch", "process", "repetition",
                         "nanos", "observations", "terminal"}, "sample fields")
    require(row["workload_id"] == name and row["operation"] == ROUTES[name][0]
            and row["dispatch"] == "hosted-scalar", "sample route")
    require(type(row["process"]) is int and 1 <= row["process"] <= 3
            and type(row["repetition"]) is int and 0 <= row["repetition"] <= 8,
            "sample coordinates")
    require(type(row["nanos"]) is int and 0 < row["nanos"] <= 30_000_000_000,
            "monotonic latency bound")
    counters = row["observations"]
    require(set(counters) == COUNTERS
            and all(type(value) is int and 0 <= value <= 2_147_483_648
                    for value in counters.values()), "selected counter fields, types or bounds")
    require(counters["rejections"] == ROUTES[name][2], "refusal count")
    require(counters["host_calls"] > 0 and counters["selected_allocations"] > 0
            and counters["sampled_budget_peak_bytes"] > 0, "missing bridge observations")
    require(counters["partial_replies"] <= counters["polls"]
            and counters["pending_polls"] <= counters["polls"]
            and counters["backpressure_pending_polls"] <= counters["pending_polls"],
            "invalid provider observations")
    require(counters["reply_retained_peak_bytes"] <= counters["sampled_budget_peak_bytes"],
            "reply charge exceeds sampled shared charge")
    require(counters["dns_queries"] == (2 if ROUTES[name][0] == "dns" else 0),
            "explicit DNS request count")
    require(counters["rollbacks"] == int(name == "tcp-read-rollback")
            and counters["cancellations"] == int(name == "pending-accept-cancel"),
            "lifecycle route")
    if name == "tcp-write-backpressure":
        require(counters["backpressure_pending_polls"] >= 16 and counters["partial_replies"] > 0,
                "fixture did not observe producer backpressure")
    else:
        require(counters["backpressure_pending_polls"] == 0, "unexpected backpressure fixture")
    useful_bytes = ROUTES[name][1]
    if ROUTES[name][0] == "tcp-write":
        require(counters["bytes_copied"] >= 2 * useful_bytes,
                "submitted argument/provider copies omitted")
    elif ROUTES[name][0] == "tcp-read":
        require(counters["bytes_copied"] == useful_bytes, "TCP output copies")
    elif ROUTES[name][0] == "udp-receive":
        require(counters["bytes_copied"] == 2 * useful_bytes, "UDP output/view copies")
    elif ROUTES[name][0] == "udp-send":
        require(counters["bytes_copied"] == 2 * useful_bytes, "UDP argument/provider copies")
    elif name in {"tls12-echo", "tls13-echo"}:
        require(counters["bytes_copied"] >= 96, "TLS application input and output copies")
    require(set(row["terminal"]) == TERMINAL
            and all(type(value) is int and value == 0 for value in row["terminal"].values()),
            "terminal ownership")


def measurement(context, name, samples):
    validate_context(context)
    for sample in samples:
        validate_sample(name, sample)
    coordinates = [(row["process"], row["repetition"]) for row in samples]
    expected = {(process, repetition) for process in range(1, 4) for repetition in range(9)}
    require(len(coordinates) == 27 and set(coordinates) == expected, "missing or duplicate samples")
    retained = sorted(samples, key=lambda row: (row["process"], row["repetition"]))
    latency = summary([row["nanos"] for row in retained])
    return {
        "workload_id": name, "operation": ROUTES[name][0], "dispatch": "hosted-scalar",
        "identity_sha256": digest({**context, "workload_id": name}),
        "samples": retained, "latency_ns": latency,
        "observations": {key: summary([row["observations"][key] for row in retained])
                         for key in sorted(COUNTERS)},
        "selected_logical_storage_estimate_bytes": summary([
            row["observations"]["selected_fixture_bytes"]
            + row["observations"]["sampled_budget_peak_bytes"] for row in retained]),
        "selected_logical_allocations": summary([
            row["observations"]["selected_allocations"]
            + row["observations"]["selected_fixture_allocations"] for row in retained]),
        "throughput_operations_per_second": 1_000_000_000 // latency["median"],
        "throughput_payload_bytes_per_second": ROUTES[name][1] * 1_000_000_000 // latency["median"],
        "native_live_handles": None,
    }


def validate_measurement(context, row):
    # JSON booleans and decimal numbers must not compare equal to derived integers.
    require(digest(row) == digest(measurement(context, row["workload_id"], row["samples"])),
            "measurement aggregation or provenance drift")


def workload(name):
    operation, payload, rejected = ROUTES[name]
    return {"id": name, "operation": operation,
            "size_class": "large" if payload >= 65536 else "small",
            "payload_bytes": payload, "expected_rejections": rejected,
            "expected_error": ERRORS.get(name), "dispatch": "hosted-scalar"}


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b"\n") and not raw.endswith(b"\n\n") and b"\r" not in raw
            and all(line.rstrip() == line for line in raw.splitlines()), "canonical whitespace")
    value = json.loads(raw)
    require(set(value) == {"format", "edition", "phase", "task", "owner", "status", "target",
            "backend", "profile", "contract", "parent_contract", "probe", "protocol", "strategy",
            "resource_model", "limits", "oracle", "identity_fields", "forbidden_identity", "workloads",
            "metrics", "report"}, "contract fields")
    require(value["format"] == "tondo-stdlib-net-performance/1" and value["edition"] == "0.1"
            and value["phase"] == "STD-0.1B" and value["task"] == "STD-NET-PERF-001"
            and value["owner"] == "std.net", "owner boundary")
    require(value["status"] in {"measurement-ready", "verified-hosted-scalar-baseline"}, "status")
    require(value["target"] == "x86_64-unknown-linux-gnu" and value["backend"] == "rust-hosted-bridge"
            and value["profile"] == "test", "target or route")
    require(value["contract"] == "docs/contracts/stdlib-net-performance.md"
            and value["parent_contract"] == "testing/stdlib-net.json", "contract paths")
    require(set(value["probe"]) == {"path", "test", "sha256"}
            and value["probe"]["path"] == PROBE
            and value["probe"]["test"] == "process_host::net::performance::network_performance_probe"
            and value["probe"]["sha256"] == hashlib.sha256((ROOT / PROBE).read_bytes()).hexdigest(),
            "probe provenance")
    require(all(digest(value[key]) == digest(expected) for key, expected in (
        ("protocol", PROTOCOL), ("strategy", STRATEGY), ("resource_model", RESOURCE_MODEL),
        ("limits", LIMITS), ("oracle", ORACLE))), "protocol, oracle or resource boundary")
    require(value["identity_fields"] == sorted(IDENTITY)
            and value["forbidden_identity"] == ["ambient_environment", "cpu_frequency", "path", "pid",
                                               "timestamp", "ephemeral_port"], "identity boundary")
    require(digest(value["workloads"]) == digest([workload(name) for name in ROUTES]),
            "workload boundary")
    require(value["metrics"] == sorted(COUNTERS | {"latency", "tail_latency", "throughput",
            "selected_logical_storage_estimate_bytes", "selected_logical_allocations", "native_live_handles"}),
            "metric boundary")
    require(digest(value["report"]) == digest({"format": "tondo-stdlib-net-performance-report/1",
            "path": "target/reliability/evidence/stdlib-net-performance.json",
            "portable_artifact": False}), "report boundary")
    return value


def measurements(context, rows):
    grouped = defaultdict(list)
    for row in rows:
        name = row["workload_id"]
        validate_sample(name, row)
        grouped[name].append(row)
    require(set(grouped) == set(ROUTES), "missing or unknown workloads")
    return [measurement(context, name, grouped[name]) for name in ROUTES]


def validate_report(contract, report, tree):
    require(set(report) == CONTEXT | {"format", "edition", "phase", "task", "owner", "capture",
            "host", "protocol", "strategy", "resource_model", "measurements"}, "report fields")
    require(report["format"] == "tondo-stdlib-net-performance-report/1", "report format")
    for key in ("edition", "phase", "task", "owner", "target", "backend", "profile", "protocol",
                "strategy", "resource_model"):
        require(digest(report[key]) == digest(contract[key]), "report contract boundary")
    context = {key: report[key] for key in CONTEXT}
    validate_context(context)
    require(context["source_tree_sha256"] == tree
            and context["probe_sha256"] == contract["probe"]["sha256"], "report source binding")
    require(report["capture"] in {"development", "clean-source"}, "capture mode")
    require(set(report["host"]) == {"cpu_model", "os"}
            and all(type(value) is str for value in report["host"].values()), "host description")
    if report["capture"] == "clean-source":
        committed = subprocess.run(["git", "-C", str(ROOT), "show",
                                    f"{context['git_revision']}:{PROBE}"], capture_output=True, check=False)
        require(committed.returncode == 0
                and hashlib.sha256(committed.stdout).hexdigest() == context["probe_sha256"],
                "capture revision lacks exact probe")
    items = report["measurements"]
    require(len(items) == len(ROUTES) and [item["workload_id"] for item in items] == list(ROUTES),
            "report workload completeness or order")
    rows = []
    for item in items:
        validate_measurement(context, item)
        rows.extend(item["samples"])
    require(items == measurements(context, rows), "retained samples or metrics drift")
    return report


def render(contract, rows, context, host, capture):
    report = {"format": "tondo-stdlib-net-performance-report/1", **context,
              **{key: contract[key] for key in ("edition", "phase", "task", "owner", "protocol",
                                               "strategy", "resource_model")},
              "host": host, "capture": capture, "measurements": measurements(context, rows)}
    return validate_report(contract, report, context["source_tree_sha256"])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("check-contract", "render", "check-report"))
    parser.add_argument("--contract", type=Path, required=True)
    parser.add_argument("--samples", type=Path)
    parser.add_argument("--report", type=Path)
    for key in ("revision", "tree-sha256", "target", "rustc", "cargo", "capture"):
        parser.add_argument("--" + key)
    for key in ("rustflags", "encoded-rustflags", "incremental", "cpu-model", "os"):
        parser.add_argument("--" + key, default="")
    args = parser.parse_args()
    contract = load_contract(args.contract)
    if args.mode == "check-contract":
        return
    require(args.report is not None and args.tree_sha256, "report/source inputs required")
    if args.mode == "check-report":
        validate_report(contract, json.loads(args.report.read_text()), args.tree_sha256)
        return
    require(args.samples is not None and args.revision and args.rustc and args.cargo and args.capture
            and args.target == contract["target"], "capture inputs")
    context = {"suite": "tondo-stdlib-net-performance", "probe_sha256": contract["probe"]["sha256"],
               "source_tree_sha256": args.tree_sha256, "git_revision": args.revision,
               "target": args.target, "backend": contract["backend"], "profile": contract["profile"],
               "toolchain": {"rustc": args.rustc, "cargo": args.cargo},
               "flags": {"rustflags": args.rustflags, "encoded_rustflags": args.encoded_rustflags,
                         "incremental": args.incremental}}
    rows = [json.loads(line) for line in args.samples.read_text().splitlines()]
    report = render(contract, rows, context, {"cpu_model": args.cpu_model, "os": args.os}, args.capture)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False) + "\n")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, AttributeError, OSError) as error:
        print(f"std.net performance: {error}", file=sys.stderr)
        sys.exit(1)
