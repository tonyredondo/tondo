#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
contract="${TONDO_STDLIB_NET_CONTRACT:-$root/testing/stdlib-net.json}"
die() { echo "std.net implementation: $*" >&2; exit 1; }
TONDO_STDLIB_NET_CONTRACT="$contract" bash scripts/stdlib-net-check.sh >/dev/null
jq -e '
  .implementation.provider_limits == {
    max_read_bytes:67108864, max_datagram_bytes:65535, max_resolver_results:1024,
    max_pending_operations:256, max_dns_jobs_per_operation:8,
    memory_metric:"finite-structural-bounds-not-vm-heap-rss-or-os-allocator-instrumentation"
  }
  and (.implementation.sources | length) == 9
  and (.implementation.sources | unique | length) == 9
  and (.implementation.tests | length) == 39
  and (.implementation.tests | unique | length) == 39
  and ([.implementation.tests[] | select(startswith("crates/tondo-stdlib/src/net.rs::"))] | length) == 14
  and ([.implementation.tests[] | select(startswith("crates/tondo-compiler/src/net_provider/tests.rs::"))] | length) == 15
  and ([.implementation.tests[] | select(startswith("crates/tondo-compiler/src/net_provider/executor.rs::"))] | length) == 5
' "$contract" >/dev/null || die "incomplete private implementation proof"
while IFS= read -r path; do
    [[ -f "$path" ]] || die "missing source: $path"
done < <(jq -r '.implementation.sources[]' "$contract")
while IFS= read -r anchor; do
    path="${anchor%%::*}"; name="${anchor##*::}"
    grep -Fq "fn $name(" "$path" || die "missing focused test: $anchor"
done < <(jq -r '.implementation.tests[]' "$contract")
python3 - <<'PY'
import tomllib
from pathlib import Path
workspace = tomllib.loads(Path('Cargo.toml').read_text())['workspace']['dependencies']
compiler = tomllib.loads(Path('crates/tondo-compiler/Cargo.toml').read_text())['dependencies']
expected = {
    'tokio': ('=1.53.2', {'rt', 'net', 'sync', 'time', 'io-util'}),
    'hickory-resolver': ('=0.26.3', {'tokio'}),
    'rustls': ('=0.23.45', {'std', 'aws_lc_rs', 'tls12'}),
}
for name, (version, features) in expected.items():
    entry = workspace[name]
    assert entry['version'] == version, name
    assert entry['default-features'] is False, name
    assert set(entry['features']) == features, name
    assert compiler[name] == {'workspace': True}, name
assert workspace['webpki-roots'] == '=1.0.9'
assert compiler['webpki-roots'] == {'workspace': True}
PY
for marker in 'ready-kernel-private-provider' 'scalar-rust-kernel-and-private-nonblocking-provider' \
    'Runtime heap admission' 'STD-NET-HOST-001' 'Native ABI/AOT'; do
    grep -Fq "$marker" docs/contracts/stdlib-net-implementation.md || die "missing boundary: $marker"
done
grep -Fq '.with_max_retries(0)' crates/tondo-compiler/src/net_provider/dns_runtime.rs || die "DNS UDP retry route changed"
grep -Fq 'pub mod net;' crates/tondo-stdlib/src/lib.rs || die "missing kernel module"
echo "std.net implementation: OK ($(jq -r '.implementation.status' "$contract"); private Rust kernel/provider only)"
