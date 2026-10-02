#!/usr/bin/env python3
"""Exact shared-corpus comparison and source-bound CBOR adapter evidence."""

import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CONTRACT = "testing/stdlib-cbor-conformance.json"
STATUS = "verified-hosted-vm-adapter-and-native-stdlib-process"
VM = "crates/tondo-reliability/examples/cbor_conformance_vm.rs"
NATIVE = "crates/tondo-native-runtime/examples/cbor_conformance.rs"
SHARED = "crates/tondo-stdlib/examples/support/cbor_conformance_cases.rs"
CORPUS = "crates/tondo-reliability/tests/fixtures/cbor-wire.json"
SOURCES = [VM, NATIVE, SHARED, CORPUS, "crates/tondo-stdlib/src/cbor.rs",
           "crates/tondo-stdlib/src/serialization.rs", "crates/tondo-vm/src/runtime.rs",
           "crates/tondo-native-runtime/src/lib.rs",
           "crates/tondo-reliability/src/cbor_model.rs",
           "crates/tondo-reliability/tests/cbor_models.rs", "Cargo.lock",
           "Cargo.toml", "rust-toolchain.toml",
           "crates/tondo-reliability/Cargo.toml", "crates/tondo-native-runtime/Cargo.toml",
           "crates/tondo-stdlib/Cargo.toml", "crates/tondo-vm/Cargo.toml",
           "scripts/stdlib_cbor_conformance.py"]
LINES = {
    "typed-dynamic": "typed-dynamic:3:18446744073709551615:2:3",
    "wire-model": "wire-model:61:30:raw-view-widths-tags",
    "deterministic": "deterministic:59:2:bytewise-shortest-nan-sign",
    "streaming": "streaming:61:270:fragments-1-2-7:closed",
    "errors-path": "errors-path:InvalidUtf8:5:7:array-0-map-0-value-tag",
    "limits-lifecycle": "limits-lifecycle:input-depth-output:atomic:terminal",
    "route-boundary": "route-boundary:scalar:public-api-not-implemented:native-abi-aot-not-claimed",
}
BOUNDARY = {
    "vm": "verified-bytecode-test-only-host-callable",
    "native": "rust-stdlib-process-no-cbor-abi",
    "compiler_api": "not-implemented", "production_host": "not-implemented",
    "native_abi": "not-implemented", "native_aot": "not-claimed",
    "simd": "not-measured-no-optimized-route",
    "runtime_objects": "table-counter-only-kernel-does-not-allocate-runtime-objects",
}
RULES = {
    "same_compiled_fixture_bytes": True, "same_case_order": True,
    "independent_wire_model_before_vm": True, "fresh_process_per_probe": True,
    "exact_returned_vm_strings": True, "exact_native_objects": True,
    "reader_fragments": [1, 2, 7], "valid_vectors": 61, "invalid_vectors": 30,
    "deterministic_outputs": 59, "key_collisions": 2, "events": 270,
    "error": {"kind": "InvalidUtf8", "start": 5, "end": 7,
              "path": ["ArrayIndex(0)", "MapEntry(0)", "MapValue", "Tag"]},
    "limits": ["input", "depth", "output"], "terminal_streams": True,
}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def digest(value):
    return sha(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())


def load_contract(path):
    raw = path.read_bytes()
    require(raw.endswith(b"\n") and b"\r" not in raw and
            all(line.rstrip() == line for line in raw.splitlines()), "contract whitespace")
    c = json.loads(raw)
    require(set(c) == {"format", "edition", "owner", "phase", "task", "status", "parent",
                       "document", "sources", "boundary", "rules", "cases", "report", "next_blocks"}, "contract fields")
    require(c["format"] == "tondo-stdlib-cbor-conformance/1" and c["edition"] == "0.1"
            and c["owner"] == "std.cbor" and c["phase"] == "STD-0.1B"
            and c["task"] == "STD-CBOR-CONF-001", "contract identity")
    require(c["status"] in {"adapter-ready", STATUS}, "contract status")
    require(c["parent"] == "testing/stdlib-cbor.json" and
            c["document"] == "docs/contracts/stdlib-cbor-conformance.md" and
            c["report"] == "target/reliability/evidence/stdlib-cbor-conformance.json" and
            c["next_blocks"] == ["STD-CBOR-DOC-001"], "contract links")
    require(c["sources"] == SOURCES and all((ROOT / p).is_file() for p in SOURCES), "shared source list")
    require(c["boundary"] == BOUNDARY and c["rules"] == RULES, "promotion or corpus boundary")
    require(c["cases"] == [{"id": key, "line": line} for key, line in LINES.items()], "case IDs or exact observables")
    return c


def compare(vm_text, native_text):
    lines = vm_text.splitlines()
    require(lines == list(LINES.values()) + ["cbor-conformance-ok"], "VM observables differ")
    native = [json.loads(line) for line in native_text.splitlines()]
    expected = [{"id": key, "status": "passed", "line": line, "cleanup": True,
                 "live_runtime_objects": 0} for key, line in LINES.items()]
    expected += [{"id": "cbor-conformance", "status": "passed"}]
    require(native == expected, "native structured observations differ")
    require(all(type(item["live_runtime_objects"]) is int and item["cleanup"] is True
                for item in native[:-1]), "native lifecycle types")
    return {"vm_stdout": lines, "native_stdout": native_text, "native_cases": native,
            "vm_log_sha256": sha(vm_text.encode()), "native_log_sha256": sha(native_text.encode())}


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, stderr=subprocess.PIPE)


def snapshot():
    return {"revision": git("rev-parse", "HEAD").decode().strip(),
            "git_tree": git("rev-parse", "HEAD^{tree}").decode().strip(),
            "dirty": bool(git("status", "--porcelain")),
            "sources": {p: sha((ROOT / p).read_bytes()) for p in SOURCES}}


def validate_report(report, contract, contract_raw, allow_development=False):
    require(set(report) == {"format", "task", "status", "qualification", "source", "contract_sha256",
                            "boundary", "comparison", "tests", "identity_sha256"}, "report fields")
    require(report["format"] == "tondo-stdlib-cbor-conformance-evidence/1" and
            report["task"] == "STD-CBOR-CONF-001" and report["status"] == "passed", "report status")
    require(report["contract_sha256"] == sha(contract_raw) and report["boundary"] == contract["boundary"], "contract binding")
    q = report["qualification"]
    require(set(q) == {"target", "toolchain", "profile", "flags"} and q["profile"] == "dev"
            and isinstance(q["target"], str) and q["target"] and
            isinstance(q["toolchain"], str) and q["toolchain"].startswith("rustc 1.93.0")
            and set(q["flags"]) == {"CARGO_INCREMENTAL"} and
            q["flags"]["CARGO_INCREMENTAL"] in {"", "0", "1"}, "target or build qualification")
    source = report["source"]
    require(set(source) == {"revision", "git_tree", "dirty", "sources"} and
            type(source["dirty"]) is bool and set(source["sources"]) == set(SOURCES)
            and re.fullmatch(r"[0-9a-f]{40}", source["revision"])
            and re.fullmatch(r"[0-9a-f]{40}", source["git_tree"]), "source fields")
    require(not source["dirty"] or allow_development, "development evidence cannot support promotion")
    for path in SOURCES:
        require(source["sources"][path] == sha((ROOT / path).read_bytes()), "current source binding: " + path)
        if not source["dirty"]:
            require(source["sources"][path] == sha(git("show", source["revision"] + ":" + path)), "committed source binding: " + path)
    require(source["git_tree"] == git("rev-parse", source["revision"] + "^{tree}").decode().strip(), "Git tree binding")
    comparison = report["comparison"]
    require(set(comparison) == {"vm_stdout", "native_stdout", "native_cases", "vm_log_sha256", "native_log_sha256"}, "comparison fields")
    normalized_vm = "\n".join(comparison["vm_stdout"]) + "\n"
    require(compare(normalized_vm, comparison["native_stdout"]) == comparison, "report comparison or log hashes")
    require(report["tests"] == {"kernel": "passed", "independent_model": "passed", "vm_dispatch": "passed"}, "test prerequisites")
    require(report["identity_sha256"] == digest({k: v for k, v in report.items() if k != "identity_sha256"}), "report identity")


def capture(contract_path, output):
    contract = load_contract(contract_path)
    contract_raw = contract_path.read_bytes()
    for key in os.environ:
        require(not (key in {"RUSTC", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET", "CARGO_BUILD_RUSTC", "CARGO_BUILD_RUSTFLAGS"}
                     or key.startswith("CARGO_PROFILE_DEV_")
                     or (key.startswith("CARGO_TARGET_") and key.endswith("_RUSTFLAGS"))) or not os.environ[key], "unsupported build override: " + key)
    before = snapshot()
    require(not before["dirty"] or os.environ.get("TONDO_STDLIB_CBOR_CONF_ALLOW_DIRTY") == "1", "clean workspace required; explicit development override only")
    logs = output.parent / "stdlib-cbor-conformance-logs"
    logs.mkdir(parents=True, exist_ok=True)
    def run(name, args):
        result = subprocess.run(["cargo", *args, "--locked"], cwd=ROOT, capture_output=True, timeout=180)
        (logs / (name + ".stdout")).write_bytes(result.stdout)
        (logs / (name + ".stderr")).write_bytes(result.stderr)
        require(result.returncode == 0, name + " failed; see retained logs")
        return result.stdout.decode()
    vm = run("vm", ["run", "-q", "-p", "tondo-reliability", "--example", "cbor_conformance_vm"])
    native = run("native", ["run", "-q", "-p", "tondo-native-runtime", "--example", "cbor_conformance"])
    comparison = compare(vm, native)
    run("kernel", ["test", "-q", "-p", "tondo-stdlib", "cbor::"])
    run("model", ["test", "-q", "-p", "tondo-reliability", "--test", "cbor_models"])
    run("dispatch", ["test", "-q", "-p", "tondo-reliability", "--example", "cbor_conformance_vm"])
    require(snapshot() == before, "source changed during capture")
    require(contract_path.read_bytes() == contract_raw, "contract changed during capture")
    toolchain = subprocess.check_output(["rustc", "-vV"], cwd=ROOT).decode().strip()
    target = next(line.removeprefix("host: ") for line in toolchain.splitlines() if line.startswith("host: "))
    report = {"format": "tondo-stdlib-cbor-conformance-evidence/1", "task": "STD-CBOR-CONF-001", "status": "passed",
              "qualification": {"target": target, "toolchain": toolchain, "profile": "dev", "flags": {"CARGO_INCREMENTAL": os.environ.get("CARGO_INCREMENTAL", "")}},
              "source": before, "contract_sha256": sha(contract_raw), "boundary": BOUNDARY,
              "comparison": comparison, "tests": {"kernel": "passed", "independent_model": "passed", "vm_dispatch": "passed"}}
    report["identity_sha256"] = digest(report)
    validate_report(report, contract, contract_raw, allow_development=before["dirty"])
    output.write_text(json.dumps(report, indent=2) + "\n")
    print("std.cbor conformance: OK (7 shared cases; 61 valid/30 invalid wires; private VM/native Rust process; " + ("development" if before["dirty"] else "clean-source") + ")")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check-contract", "capture", "check-report"])
    parser.add_argument("--contract", type=pathlib.Path, default=ROOT / CONTRACT)
    parser.add_argument("--report", type=pathlib.Path)
    parser.add_argument("--allow-development", action="store_true")
    args = parser.parse_args()
    contract = load_contract(args.contract)
    if args.action == "capture":
        require(args.report is not None, "report path required")
        capture(args.contract, args.report)
    elif args.action == "check-report":
        require(args.report is not None, "report path required")
        validate_report(json.loads(args.report.read_text()), contract, args.contract.read_bytes(), args.allow_development)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.SubprocessError) as error:
        print("std.cbor conformance: " + str(error), file=sys.stderr)
        sys.exit(1)
