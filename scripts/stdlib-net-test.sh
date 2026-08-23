#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/tondo-stdlib-net-negative.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

expect_failure() {
    local name="$1"
    shift
    if "$@" >/dev/null 2>&1; then
        echo "std.net tests: $name unexpectedly passed" >&2
        exit 1
    fi
}

jq '.capabilities.required = ["filesystem"]' testing/stdlib-net.json > "$tmp_dir/wrong-capability.json"
expect_failure wrong-capability env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/wrong-capability.json" scripts/stdlib-net-check.sh

jq '.surface.selectable_operations = ["listener-accept", "tcp-write", "udp-receive"]' testing/stdlib-net.json > "$tmp_dir/write-selectable.json"
expect_failure write-selectable env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/write-selectable.json" scripts/stdlib-net-check.sh

jq '.dns.connect_resolves = true' testing/stdlib-net.json > "$tmp_dir/implicit-dns.json"
expect_failure implicit-dns env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/implicit-dns.json" scripts/stdlib-net-check.sh

jq '.tcp.write_unknown_partial_error = true' testing/stdlib-net.json > "$tmp_dir/unknown-partial.json"
expect_failure unknown-partial env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/unknown-partial.json" scripts/stdlib-net-check.sh

jq '.tls.insecure_mode = true' testing/stdlib-net.json > "$tmp_dir/insecure-tls.json"
expect_failure insecure-tls env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/insecure-tls.json" scripts/stdlib-net-check.sh

jq '.cancellation.cleanup_before_scope_exit = false' testing/stdlib-net.json > "$tmp_dir/cleanup-gap.json"
expect_failure cleanup-gap env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/cleanup-gap.json" scripts/stdlib-net-check.sh

jq '.negative_cases += ["duplicate-negative"]' testing/stdlib-net.json > "$tmp_dir/duplicate-negative.json"
expect_failure duplicate-negative env TONDO_STDLIB_NET_CONTRACT="$tmp_dir/duplicate-negative.json" scripts/stdlib-net-check.sh

for marker in \
    'pub enum NetError' \
    'pub enum TlsError' \
    'pub enum TlsVerification' \
    'pub fn NetLimits.create(maxRead: Int, maxDatagram: Int, maxResults: Int): NetLimits ! NetError' \
    'pub fn options(deadline: Instant?, limits: NetLimits): NetOptions ! NetError' \
    'pub fn connect(address: SocketAddress, options: NetOptions): TcpStream ! NetError suspends' \
    'pub fn TcpStream.split(self): (TcpReadHalf, TcpWriteHalf)' \
    'pub fn TcpStream.shutdown(ref self, how: Shutdown, options: NetOptions): Unit ! NetError suspends' \
    'pub fn UdpSocket.sendTo(ref self, data: Bytes, destination: SocketAddress, options: NetOptions): Unit ! NetError suspends' \
    'pub fn TlsStream.connect(stream: TcpStream, server: HostName, config: TlsConfig, options: NetOptions): TlsStream ! TlsError suspends'; do
    grep -Fq "$marker" docs/contracts/stdlib-net.md || { echo "missing marker: $marker" >&2; exit 1; }
done

for marker in \
    'implicit-retry' \
    'implicit-happy-eyeballs' \
    'DatagramTooLarge-and-no-truncation' \
    'target-declared-and-hash-pinned' \
    'cleanup_before_scope_exit' \
    'diagnostics-publish-payloads'; do
    grep -Fq "$marker" testing/stdlib-net.json || { echo "missing contract anchor: $marker" >&2; exit 1; }
done

jq -e '
  .task == "STD-NET-001"
  and .capabilities.required == ["network"]
  and .capabilities.optional == ["clock"]
  and .options.foreign_clock_domain == "NetError.InvalidDeadline"
  and .surface.selectable_operations == ["listener-accept", "tcp-read", "udp-receive"]
  and .dns.connect_resolves == false
  and .tcp.write_partial_count == true
  and .udp.oversize == "DatagramTooLarge-and-no-truncation"
  and .tls.insecure_mode == false
  and .tls.plaintext_downgrade == false
  and .cancellation.cleanup_before_scope_exit == true
  and .implementation.public_api_promoted == false
' testing/stdlib-net.json >/dev/null

echo "std.net tests: OK (negative capability, deadline, ownership, partial-I/O, UDP, TLS and cleanup anchors)"
