#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
python3 - <<'PY'
import hashlib
import json
import re
from pathlib import Path

source = Path("stdlib/meta/src/meta.to")
destination = Path("stdlib/meta/descriptor.json")
owner_path = Path("testing/stdlib-meta.json")

def encode(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode()

def digest(value):
    return "sha256:" + hashlib.sha256(value).hexdigest()

content = {
    "api": "tondo-std-meta-0.1/1",
    "format": "tondo-std-meta-package-draft",
    "package_id": "toolchain:std-meta:draft",
    "profile": "meta",
    "source": {
        "logical_path": "src/meta.to",
        "module": "meta",
        "sha256": digest(source.read_bytes()),
    },
    "target": "tondo-meta",
}
descriptor = dict(sorted({**content, "content_hash": digest(encode(content))}.items()))
owner_source = owner_path.read_text()
owner = json.loads(owner_source)
owner["package"]["source_hash"] = content["source"]["sha256"]
owner["package"]["content_hash"] = descriptor["content_hash"]
for field in ("source_hash", "content_hash"):
    pattern = rf'("{field}"\s*:\s*)"[^"]*"'
    owner_source, count = re.subn(
        pattern, lambda match: match[1] + json.dumps(owner["package"][field]), owner_source
    )
    if count != 1:
        raise ValueError(f"owner must have exactly one {field} pin")
if json.loads(owner_source) != owner:
    raise ValueError("owner pin update changed unrelated contract data")
destination.write_bytes(encode(descriptor) + b"\n")
owner_path.write_text(owner_source)
print("std.meta package descriptor and owner pins: generated")
PY
