"""Capture and verify the exact local inputs used by a VM distribution."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def source_state(root):
    def git(*arguments):
        return subprocess.check_output(["git", "-C", str(root), *arguments])

    paths = sorted(set(git("ls-files", "--cached", "--others", "--exclude-standard", "-z")
                       .decode("utf-8").rstrip("\0").split("\0")))
    return {
        "revision": git("rev-parse", "HEAD").decode().strip(),
        "dirty": bool(git("status", "--porcelain=v1", "--untracked-files=all", "-z")),
        "paths": [path for path in paths if path and
                  ((root / path).exists() or (root / path).is_symlink())],
    }


def source_path(root, name):
    path = Path(name)
    if (path.is_absolute() or not path.parts or path.as_posix() != name
            or any(part in (".", "..") for part in path.parts)):
        raise ValueError("distribution source path must be relative and canonical")
    current = root
    for part in path.parts:
        current = current / part
        if current.is_symlink():
            raise ValueError("distribution inputs must not contain symbolic links")
    return current


def file_record(path):
    mode = path.lstat().st_mode
    if not stat.S_ISREG(mode):
        raise ValueError("distribution input must be a regular file")
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
            size += len(chunk)
    return {"sha256": digest.hexdigest(), "bytes": size,
            "mode": 0o755 if mode & 0o111 else 0o644}


def metadata(root, binary, contract, state):
    files = [{"path": name, **file_record(source_path(root, name))}
             for name in sorted(set(state["paths"]))]
    return {
        "format": "tondo-stdlib-distribution-inputs/1",
        "source_revision": state["revision"],
        "source_dirty": state["dirty"],
        "source_tree_sha256": hashlib.sha256(canonical(files)).hexdigest(),
        "source_files": files,
        "binary": file_record(binary),
        "contract": file_record(contract),
        "binary_source_provenance": "not-claimed",
    }


def capture(root, destination, binary, contract, state):
    record = metadata(root, binary, contract, state)
    for entry in record["source_files"]:
        output = destination / "source" / entry["path"]
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_path(root, entry["path"]), output)
        output.chmod(entry["mode"])
        if file_record(output) != {key: entry[key] for key in ("sha256", "bytes", "mode")}:
            raise ValueError("distribution source changed during capture")
    for source, relative, key in [(binary, "runtime/tondo", "binary"),
                                  (contract, "contract.json", "contract")]:
        output = destination / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, output)
        output.chmod(record[key]["mode"])
        if file_record(output) != record[key]:
            raise ValueError("distribution binary or contract changed during capture")
    (destination / "inputs.json").write_bytes(canonical(record) + b"\n")
    return record


def verify(root, destination, binary, contract, state):
    expected = json.loads((destination / "inputs.json").read_bytes())
    if expected != metadata(root, binary, contract, state):
        raise ValueError("distribution inputs changed after capture")
    captured_state = {"revision": state["revision"], "dirty": state["dirty"],
                      "paths": state["paths"]}
    actual = metadata(destination / "source", destination / "runtime/tondo",
                      destination / "contract.json", captured_state)
    if expected != actual:
        raise ValueError("captured distribution inputs changed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("capture", "verify"))
    for name in ("root", "destination", "binary", "contract"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    state = source_state(args.root)
    if state["dirty"] and not args.allow_dirty:
        raise ValueError("workspace must be clean; dirty capture is local iteration evidence only")
    if args.operation == "capture":
        capture(args.root, args.destination, args.binary, args.contract, state)
    verify(args.root, args.destination, args.binary, args.contract, source_state(args.root))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        raise SystemExit(f"stdlib distribution inputs: {error}") from error
