#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

contract="${TONDO_STDLIB_NET_CONTRACT:-$root/testing/stdlib-net.json}"

die() {
    echo "std.net contract: $*" >&2
    exit 1
}

[[ -f "$contract" ]] || die "missing owner contract: ${contract#"$root/"}"
tail -c 1 "$contract" | cmp -s <(printf '\n') || die "owner contract must end with LF"
! grep -nE $'\r|[[:blank:]]$' "$contract" >/dev/null || die "owner contract contains CR or trailing whitespace"

jq -e '
  .format == "tondo-stdlib-owner-contract/1"
  and .owner == "std.net"
  and .parent_owner == "std"
  and .edition == "0.1"
  and .phase == "STD-0.1B"
  and .task == "STD-NET-001"
  and .status == "contract-locked"
  and .contract == "docs/contracts/stdlib-net.md"
  and .spec == "TONDO_STANDARD_LIBRARY_SPEC.md"
  and .language_spec == "TONDO_LANGUAGE_SPEC.md"
  and .layer == "B0"
  and .kind == "runtime-facing"
  and .target == "tondo-vm-hosted-and-native"
  and .capabilities.required == ["network"]
  and .capabilities.optional == ["clock"]
  and .capabilities.threads == "forbidden-as-public-requirement"
  and .capabilities.ambient_lookup == false
  and .capabilities.import_effect == "none"
  and .capabilities.missing_network == "static-capability-error"
  and .capabilities.deadline_without_clock == "static-capability-error"
  and .host.status == "required-after-native-gate"
  and .surface.types[0:5] == ["HostName", "IpAddress", "SocketAddress", "NetLimits", "NetOptions"]
  and (.surface.signatures | length) == 39
  and ([.surface.signatures[].id] | unique | length) == 39
  and any(.surface.signatures[]; .id == "tcp-shutdown" and .signature == "pub fn TcpStream.shutdown(ref self, how: Shutdown, options: NetOptions): Unit ! NetError suspends")
  and all(.surface.signatures[]; (.signature | type == "string" and length > 0) and (.kind | type == "string" and length > 0) and (.effect | type == "string" and length > 0))
  and .surface.direct_call_waits == true
  and .surface.explicit_await_direct_call == "forbidden"
  and .surface.explicit_await_join == "required"
  and .surface.inference_by_name == false
  and .surface.bodyless_requires_effect == true
  and .surface.bodyful_inference == "allowed"
  and .surface.selectable_operations == ["listener-accept", "tcp-read", "udp-receive"]
  and .ownership.address_values_copy == true
  and .ownership.options_copy == true
  and .ownership.listener_affine == true
  and .ownership.stream_affine == true
  and .ownership.socket_affine == true
  and .ownership.handles_copy == false
  and .ownership.handles_share == false
  and .ownership.handles_send == true
  and .ownership.split_consumes_stream == true
  and .ownership.half_affine == true
  and .ownership.last_half_closes_transport == true
  and .ownership.implicit_drop == "compile-error"
  and .ownership.post_close_use == "compile-error"
  and .ownership.select_loser_resource == "resource-remains-owned-and-unconsumed"
  and .limits.port == "0..65535; zero-bind-only"
  and .limits.no_unbounded_tondo_buffer == true
  and .options.deadline == "explicit-monotonic-Instant-or-none"
  and .options.deadline_requires == "clock-capability"
  and .options.foreign_clock_domain == "NetError.InvalidDeadline"
  and .options.ambient_timeout == "forbidden"
  and .options.ambient_proxy == "forbidden"
  and .options.ambient_retry == "forbidden"
  and .dns.connect_resolves == false
  and .dns.provider_order == "preserved-after-bytewise-deduplication"
  and .dns.automatic_retry == false
  and .dns.happy_eyeballs == "caller-composed-with-spawn-and-Group"
  and .tcp.listener_accept_selectable == true
  and .tcp.read_partial == true
  and .tcp.read_eof == "ReadResult.Eof"
  and .tcp.write_partial_count == true
  and .tcp.write_unknown_partial_error == false
  and .tcp.write_unbounded_buffer == false
  and .tcp.split == "one-affine-read-half-and-one-affine-write-half"
  and .udp.send_atomic_datagram == true
  and .udp.receive_selectable == true
  and .udp.oversize == "DatagramTooLarge-and-no-truncation"
  and .tls.provider == "target-declared-and-hash-pinned"
  and .tls.verification_default == "PlatformRoots"
  and .tls.insecure_mode == false
  and .tls.plaintext_downgrade == false
  and .tls.server_name_verification == true
  and .tls.handshake_publishes_stream_after_success == true
  and .tls.handshake_failure_closes_transport == true
  and .tls.renegotiation == "forbidden"
  and .cancellation.all_suspendible_operations == true
  and .cancellation.waiting_accept == "unregister-before-commit"
  and .cancellation.waiting_read == "unregister-before-byte-consume"
  and .cancellation.waiting_receive == "unregister-before-datagram-consume"
  and .cancellation.cleanup_before_scope_exit == true
  and .cancellation.no_detached_host_resources == true
  and .diagnostics.event_namespace == "std.net"
  and (.diagnostics.events | length) == 16
  and .diagnostics.required_fields == ["run_id", "task_id", "operation_id", "resource_id", "event_sequence", "state", "source_revision", "target"]
  and .diagnostics.payloads == "omitted-by-default"
  and .diagnostics.runtime_hooks_public == false
  and .portability.target_declared == ["ipv4", "ipv6", "resolver", "readiness", "tls-provider", "trust-bundle", "socket-limits"]
  and ((.exclusions | unique | length) == (.exclusions | length))
  and ((.negative_cases | unique | length) == (.negative_cases | length))
  and (.negative_cases | length) == 40
  and .implementation.status == "pending-after-native-gate"
  and .implementation.public_api_promoted == false
  and .implementation.host == "required-after-native-gate"
  and .implementation.required_follow_ups == ["STD-NET-IMPL-001", "STD-NET-HOST-001", "STD-NET-TEST-001", "STD-NET-PERF-001", "STD-NET-CONF-001", "STD-NET-DOC-001"]
  and .promotion.next_blocks == ["STD-REGEX-001"]
' "$contract" >/dev/null || die "invalid machine-readable net contract"

for path in \
    docs/contracts/stdlib-net.md \
    TONDO_STANDARD_LIBRARY_SPEC.md \
    TONDO_IMPLEMENTATION_TRACKER.md; do
    [[ -f "$root/$path" ]] || die "missing linked contract: $path"
done

for marker in \
    'STD-NET-001' \
    'pub type TcpStream' \
    'pub fn resolve(host: HostName, port: Int, options: NetOptions): Array[SocketAddress] ! NetError suspends' \
    'pub fn TcpListener.accept(ref self, options: NetOptions): TcpStream ! NetError selectable' \
    'pub fn TcpReadHalf.read(ref self, max: Int, options: NetOptions): ReadResult ! NetError selectable' \
    'pub fn UdpSocket.receiveFrom(ref self, options: NetOptions): Datagram ! NetError selectable' \
    'PlatformRoots' \
    'DatagramTooLarge' \
    'required-after-native-gate'; do
    grep -Fq "$marker" "$root/docs/contracts/stdlib-net.md" || die "contract document misses marker: $marker"
done

grep -Fq 'testing/stdlib-net.json' "$root/TONDO_STANDARD_LIBRARY_SPEC.md" || die "main stdlib spec does not link the net registry"

echo "std.net contract: OK (capability-gated addresses; DNS; TCP/UDP; TLS boundary; deadlines and cleanup)"
