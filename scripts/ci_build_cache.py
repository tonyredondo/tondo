"""Bounded third-party Cargo artifacts; execution evidence is never cached."""

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
MAX_BYTES = 4 * 1024**3
FORMAT = "tondo-ci-dependency-cache/1"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            value.update(block)
    return value.hexdigest()


def signature():
    return {
        "lock": digest(ROOT / "Cargo.lock"), "toolchain_file": digest(ROOT / "rust-toolchain.toml"),
        "rustc": subprocess.check_output(["rustc", "-vV"], text=True).strip(),
        "flags": {key: os.environ.get(key, "") for key in (
            "CARGO_INCREMENTAL", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET")},
    }


def regular_files(path):
    require(not path.is_symlink(), "symlink in dependency artifact")
    if path.is_file():
        yield path
    elif path.is_dir():
        for file in sorted(path.rglob("*")):
            require(not file.is_symlink(), "symlink in dependency artifact")
            if file.is_file():
                yield file


def select(target, packages, maximum=MAX_BYTES):
    groups = []
    fingerprints = target / "debug/.fingerprint"
    for directory in fingerprints.glob("*"):
        match = re.fullmatch(r"(.+)-([0-9a-f]{16})", directory.name)
        if not match or match[1] not in packages:
            continue
        package, unit = match.groups()
        paths = set(regular_files(directory))
        for file in (target / "debug/deps").glob(f"*-{unit}.*"):
            paths.update(regular_files(file))
        paths.update(regular_files(target / "debug/build" / f"{package}-{unit}"))
        paths = sorted(paths)
        if not paths:
            continue
        groups.append((max(file.stat().st_mtime_ns for file in paths), paths))
    groups.sort(key=lambda group: group[0], reverse=True)
    selected = set()
    size = 0
    for _, paths in groups:
        extra = [path for path in paths if path not in selected]
        amount = sum(path.stat().st_size for path in extra)
        if size + amount <= maximum:
            selected.update(extra)
            size += amount
    return sorted(selected), size


def snapshot(target, cache):
    require(not cache.exists(), "cache staging directory already exists")
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT, text=True))
    packages = {package["name"] for package in metadata["packages"]
                if package.get("source") and package["source"].startswith(("registry+", "git+"))}
    paths, size = select(target, packages)
    cache.mkdir(parents=True)
    entries = []
    for source in paths:
        relative = source.relative_to(target)
        destination = cache / "payload" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        entries.append({"path": relative.as_posix(), "size": source.stat().st_size,
                        "sha256": digest(destination)})
    value = {"format": FORMAT, "signature": signature(), "bytes": size, "entries": entries}
    (cache / "manifest.json").write_text(json.dumps(value, indent=2) + "\n")
    print(f"CI dependency cache: staged {len(entries)} files, {size} bytes (limit {MAX_BYTES})")


def validated(cache, expected):
    value = json.loads((cache / "manifest.json").read_text())
    require(set(value) == {"format", "signature", "bytes", "entries"}
            and value["format"] == FORMAT and value["signature"] == expected, "cache build identity")
    require(type(value["bytes"]) is int and 0 <= value["bytes"] <= MAX_BYTES, "cache byte budget")
    require(type(value["entries"]) is list, "cache entries")
    paths = set()
    total = 0
    for entry in value["entries"]:
        require(type(entry) is dict and set(entry) == {"path", "size", "sha256"}, "cache entry fields")
        relative = Path(entry["path"])
        require(not relative.is_absolute() and ".." not in relative.parts
                and len(relative.parts) >= 3 and relative.parts[:2] in {
                    ("debug", ".fingerprint"), ("debug", "deps"), ("debug", "build")}, "cache path boundary")
        require(relative.as_posix() not in paths and "incremental" not in relative.parts,
                "duplicate or incremental artifact")
        if relative.parts[1] == "deps":
            require(relative.suffix in {".d", ".rlib", ".rmeta", ".so", ".a"},
                    "only dependency libraries and dep-info belong in the cache")
        paths.add(relative.as_posix())
        file = cache / "payload" / relative
        require(all(not parent.is_symlink() for parent in (file, *file.parents)), "cache symlink")
        require(type(entry["size"]) is int and entry["size"] >= 0
                and file.stat().st_size == entry["size"] and digest(file) == entry["sha256"], "cache artifact changed")
        total += entry["size"]
    require(total == value["bytes"], "cache total bytes")
    return value


def restore(target, cache):
    value = validated(cache, signature())
    for entry in value["entries"]:
        destination = target / entry["path"]
        require(all(not parent.is_symlink() for parent in (destination, *destination.parents)),
                "build destination symlink")
        require(not destination.exists(), "build cache must be restored before compilation")
        require(all(not parent.exists() or parent.is_dir() for parent in destination.parents),
                "build destination parent is not a directory")
    for entry in value["entries"]:
        source = cache / "payload" / entry["path"]
        destination = target / entry["path"]
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    print(f"CI dependency cache: restored {len(value['entries'])} validated files")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("snapshot", "restore"))
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    args = parser.parse_args()
    if args.mode == "snapshot":
        snapshot(args.target, args.cache)
    elif (args.cache / "manifest.json").exists():
        restore(args.target, args.cache)
    else:
        print("CI dependency cache: cold build")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        print(f"CI dependency cache: {error}", file=sys.stderr)
        sys.exit(1)
