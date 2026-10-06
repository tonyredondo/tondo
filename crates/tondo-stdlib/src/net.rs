//! Host-independent networking values and argument admission.
//!
//! This kernel does not open sockets, resolve names or perform TLS. The compiler
//! provider owns those effects and must validate arguments before registering
//! work. Kernel types and tests do not establish a public Tondo or native ABI.

use std::{fmt, net::IpAddr};

use crate::io::ReadResult;

pub const MAX_HOST_NAME_BYTES: usize = 253;
pub const MAX_HOST_LABEL_BYTES: usize = 63;
pub const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_DATAGRAM_BYTES: usize = 65_535;
pub const MAX_RESOLVER_RESULTS: usize = 1_024;

/// Declaration order of the Tondo error enum, not a native ABI.
pub const ERROR_VARIANTS: &[&str] = &[
    "InvalidHostName",
    "InvalidAddress",
    "InvalidPort",
    "InvalidLimit",
    "InvalidDeadline",
    "ResourceLimit",
    "ResolveFailed",
    "ConnectionRefused",
    "ConnectionReset",
    "NotConnected",
    "AddressInUse",
    "Unreachable",
    "DatagramTooLarge",
    "Timeout",
    "Cancelled",
    "Closed",
    "CapabilityMissing",
    "Host",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetError {
    InvalidHostName,
    InvalidAddress,
    InvalidPort,
    InvalidLimit,
    InvalidDeadline,
    ResourceLimit,
    ResolveFailed,
    ConnectionRefused,
    ConnectionReset,
    NotConnected,
    AddressInUse,
    Unreachable,
    DatagramTooLarge,
    Timeout,
    Cancelled,
    Closed,
    CapabilityMissing,
    Host,
}

impl NetError {
    pub const fn variant(self) -> u32 {
        self as u32
    }
}

impl fmt::Display for NetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for NetError {}

/// TLS errors retain nominal transport failures without platform details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsError {
    InvalidServerName,
    InvalidCertificate,
    CertificateRejected,
    HandshakeFailed,
    Unsupported,
    ResourceLimit,
    Timeout,
    Cancelled,
    Closed,
    Transport(NetError),
}

impl fmt::Display for TlsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for TlsError {}

/// A checked ASCII host name with fixed storage and bytewise value identity.
/// Its original case and optional final root dot are retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostName {
    bytes: [u8; MAX_HOST_NAME_BYTES],
    length: u8,
}

impl HostName {
    pub fn parse(text: &str) -> Result<Self, NetError> {
        if text.is_empty() || text.len() > MAX_HOST_NAME_BYTES || !text.is_ascii() {
            return Err(NetError::InvalidHostName);
        }
        let labels = text.strip_suffix('.').unwrap_or(text);
        for label in labels.split('.') {
            let bytes = label.as_bytes();
            if bytes.is_empty()
                || bytes.len() > MAX_HOST_LABEL_BYTES
                || !bytes[0].is_ascii_alphanumeric()
                || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
                || !bytes
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
            {
                return Err(NetError::InvalidHostName);
            }
        }
        let mut bytes = [0; MAX_HOST_NAME_BYTES];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(Self {
            bytes,
            length: text.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = std::str::from_utf8(self.as_bytes()).map_err(|_| fmt::Error)?;
        formatter.write_str(text)
    }
}

/// Numeric addresses preserve their family, including IPv4-mapped IPv6.
/// Parsing accepts no DNS name, port, zone ID or surrounding whitespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IpAddress(IpAddr);

impl IpAddress {
    pub fn parse(text: &str) -> Result<Self, NetError> {
        text.parse().map(Self).map_err(|_| NetError::InvalidAddress)
    }

    pub const fn from_ip(ip: IpAddr) -> Self {
        Self(ip)
    }

    pub const fn as_ip(self) -> IpAddr {
        self.0
    }
}

impl fmt::Display for IpAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SocketAddress {
    ip: IpAddress,
    port: u16,
}

impl SocketAddress {
    /// Port zero is a valid value, usable only by bind/listen operations.
    pub fn create(ip: IpAddress, port: i128) -> Result<Self, NetError> {
        let port = u16::try_from(port).map_err(|_| NetError::InvalidPort)?;
        Ok(Self { ip, port })
    }

    pub const fn ip(self) -> IpAddress {
        self.ip
    }

    pub const fn port(self) -> u16 {
        self.port
    }

    pub const fn require_destination(self) -> Result<Self, NetError> {
        if self.port == 0 {
            Err(NetError::InvalidPort)
        } else {
            Ok(self)
        }
    }
}

/// Fixed target ceilings; providers may impose a smaller remaining run budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetLimits {
    max_read: usize,
    max_datagram: usize,
    max_results: usize,
}

impl NetLimits {
    pub fn create(max_read: i128, max_datagram: i128, max_results: i128) -> Result<Self, NetError> {
        if max_read <= 0 || max_datagram <= 0 || max_results <= 0 {
            return Err(NetError::InvalidLimit);
        }
        let max_read = usize::try_from(max_read).map_err(|_| NetError::ResourceLimit)?;
        let max_datagram = usize::try_from(max_datagram).map_err(|_| NetError::ResourceLimit)?;
        let max_results = usize::try_from(max_results).map_err(|_| NetError::ResourceLimit)?;
        if max_read > MAX_READ_BYTES
            || max_datagram > MAX_DATAGRAM_BYTES
            || max_results > MAX_RESOLVER_RESULTS
        {
            return Err(NetError::ResourceLimit);
        }
        Ok(Self {
            max_read,
            max_datagram,
            max_results,
        })
    }

    pub const fn max_read(self) -> usize {
        self.max_read
    }

    pub const fn max_datagram(self) -> usize {
        self.max_datagram
    }

    pub const fn max_results(self) -> usize {
        self.max_results
    }

    pub fn read_request(self, requested: i128) -> Result<usize, NetError> {
        if requested <= 0 {
            return Err(NetError::InvalidLimit);
        }
        let requested = usize::try_from(requested).map_err(|_| NetError::ResourceLimit)?;
        if requested > self.max_read {
            return Err(NetError::ResourceLimit);
        }
        Ok(requested)
    }

    pub const fn datagram_size(self, bytes: usize) -> Result<(), NetError> {
        if bytes > self.max_datagram {
            Err(NetError::DatagramTooLarge)
        } else {
            Ok(())
        }
    }
}

impl Default for NetLimits {
    fn default() -> Self {
        Self {
            max_read: 64 * 1024,
            max_datagram: MAX_DATAGRAM_BYTES,
            max_results: 128,
        }
    }
}

/// A bridge snapshot of std.time's opaque provider-domain instant.
/// Negative ticks are valid instants before the provider origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    domain: u64,
    nanos: i64,
}

impl Deadline {
    pub fn create(domain: u64, nanos: i128) -> Result<Self, NetError> {
        if domain == 0 {
            return Err(NetError::InvalidDeadline);
        }
        let nanos = i64::try_from(nanos).map_err(|_| NetError::InvalidDeadline)?;
        Ok(Self { domain, nanos })
    }

    pub const fn domain(self) -> u64 {
        self.domain
    }

    pub const fn nanos(self) -> i64 {
        self.nanos
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetOptions {
    deadline: Option<Deadline>,
    limits: NetLimits,
}

impl NetOptions {
    /// Construction uses an explicit domain and never reads a clock.
    pub fn create(
        deadline: Option<Deadline>,
        limits: NetLimits,
        active_domain: u64,
    ) -> Result<Self, NetError> {
        if deadline.is_some_and(|deadline| deadline.domain != active_domain) {
            return Err(NetError::InvalidDeadline);
        }
        Ok(Self { deadline, limits })
    }

    pub const fn deadline(self) -> Option<Deadline> {
        self.deadline
    }

    pub const fn limits(self) -> NetLimits {
        self.limits
    }

    /// Check before commit: invalid domain, then cancellation, then timeout.
    /// The provider must not apply this check after an operation commits.
    pub fn before_commit(
        self,
        active_domain: u64,
        now_nanos: i128,
        cancelled: bool,
    ) -> Result<(), NetError> {
        if self.deadline.is_some_and(|deadline| {
            deadline.domain != active_domain || i64::try_from(now_nanos).is_err()
        }) {
            return Err(NetError::InvalidDeadline);
        }
        if cancelled {
            return Err(NetError::Cancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| i128::from(deadline.nanos) <= now_nanos)
        {
            return Err(NetError::Timeout);
        }
        Ok(())
    }
}

/// Preserve provider order after bytewise identity deduplication. Refuse an
/// oversized provider snapshot or final result rather than publishing a prefix.
pub fn resolver_results(
    addresses: &[SocketAddress],
    limits: NetLimits,
) -> Result<Vec<SocketAddress>, NetError> {
    if addresses.len() > MAX_RESOLVER_RESULTS {
        return Err(NetError::ResourceLimit);
    }
    let mut result = Vec::new();
    result
        .try_reserve_exact(addresses.len().min(limits.max_results))
        .map_err(|_| NetError::ResourceLimit)?;
    for address in addresses {
        address.require_destination()?;
        if result.contains(address) {
            continue;
        }
        if result.len() == limits.max_results {
            return Err(NetError::ResourceLimit);
        }
        result.push(*address);
    }
    Ok(result)
}

/// Admit a provider read without publishing an empty or oversized Data value.
/// A final nonempty chunk is delivered before EOF, never replaced by it.
pub fn tcp_read_result(
    bytes: Vec<u8>,
    requested: usize,
    eof: bool,
) -> Result<ReadResult, NetError> {
    if requested == 0 || bytes.len() > requested {
        return Err(NetError::Host);
    }
    if bytes.is_empty() {
        return if eof {
            Ok(ReadResult::Eof)
        } else {
            Err(NetError::Host)
        };
    }
    Ok(ReadResult::Data(bytes))
}

/// Accept exactly the reported prefix. A nonempty write returning zero cannot
/// be represented as progress; WouldBlock must remain pending in the provider.
pub const fn tcp_write_result(requested: usize, reported: usize) -> Result<usize, NetError> {
    if reported > requested || (requested != 0 && reported == 0) {
        Err(NetError::Host)
    } else {
        Ok(reported)
    }
}

/// A successful UDP send must accept the whole datagram, including empty ones.
pub const fn udp_send_result(requested: usize, reported: usize) -> Result<(), NetError> {
    if reported == requested {
        Ok(())
    } else {
        Err(NetError::Host)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    bytes: Vec<u8>,
    source: SocketAddress,
}

impl Datagram {
    /// Refuse oversize or truncated input without retaining a partial value.
    /// The provider supplies the wire length, including discarded bytes.
    pub fn receive(
        bytes: Vec<u8>,
        source: SocketAddress,
        wire_length: usize,
        limits: NetLimits,
    ) -> Result<Self, NetError> {
        limits.datagram_size(wire_length)?;
        if bytes.len() != wire_length || source.port == 0 {
            return Err(NetError::Host);
        }
        Ok(Self { bytes, source })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn source(&self) -> SocketAddress {
        self.source
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(text: &str, port: i128) -> SocketAddress {
        SocketAddress::create(IpAddress::parse(text).unwrap(), port).unwrap()
    }

    #[test]
    fn tcp_read_admission_delivers_the_final_chunk_before_eof() {
        assert_eq!(
            tcp_read_result(vec![1, 2], 4, false),
            Ok(ReadResult::Data(vec![1, 2]))
        );
        assert_eq!(
            tcp_read_result(vec![3], 4, true),
            Ok(ReadResult::Data(vec![3]))
        );
        assert_eq!(tcp_read_result(vec![], 4, true), Ok(ReadResult::Eof));
        assert_eq!(tcp_read_result(vec![], 4, false), Err(NetError::Host));
        assert_eq!(tcp_read_result(vec![1], 0, false), Err(NetError::Host));
        assert_eq!(tcp_read_result(vec![1, 2], 1, false), Err(NetError::Host));
    }

    #[test]
    fn tcp_write_admission_keeps_partial_progress_exact() {
        assert_eq!(tcp_write_result(4, 2), Ok(2));
        assert_eq!(tcp_write_result(4, 4), Ok(4));
        assert_eq!(tcp_write_result(0, 0), Ok(0));
        assert_eq!(tcp_write_result(4, 0), Err(NetError::Host));
        assert_eq!(tcp_write_result(4, 5), Err(NetError::Host));
        assert_eq!(tcp_write_result(0, 1), Err(NetError::Host));
    }

    #[test]
    fn udp_send_admission_never_reports_a_partial_datagram_as_success() {
        assert_eq!(udp_send_result(4, 4), Ok(()));
        assert_eq!(udp_send_result(0, 0), Ok(()));
        assert_eq!(udp_send_result(4, 3), Err(NetError::Host));
        assert_eq!(udp_send_result(4, 5), Err(NetError::Host));
    }

    #[test]
    fn received_datagrams_preserve_empty_payloads_and_exact_source() {
        let source = address("::1", 1234);
        let limits = NetLimits::create(1, 3, 1).unwrap();
        let datagram = Datagram::receive(vec![0, 255], source, 2, limits).unwrap();
        assert_eq!(datagram.bytes(), &[0, 255]);
        assert_eq!(datagram.source(), source);
        assert_eq!(datagram.into_bytes(), vec![0, 255]);
        assert!(
            Datagram::receive(vec![], source, 0, limits)
                .unwrap()
                .bytes()
                .is_empty()
        );
    }

    #[test]
    fn received_datagrams_refuse_truncation_and_oversize_without_partial_values() {
        let source = address("127.0.0.1", 1234);
        let limits = NetLimits::create(1, 3, 1).unwrap();
        assert_eq!(
            Datagram::receive(vec![0, 1], source, 4, limits),
            Err(NetError::DatagramTooLarge)
        );
        assert_eq!(
            Datagram::receive(vec![0, 1], source, 3, limits),
            Err(NetError::Host)
        );
        assert_eq!(
            Datagram::receive(vec![0, 1, 2], source, 2, limits),
            Err(NetError::Host)
        );
        assert_eq!(
            Datagram::receive(vec![], address("::1", 0), 0, limits),
            Err(NetError::Host)
        );
    }

    #[test]
    fn nominal_errors_never_include_host_or_certificate_payloads() {
        assert_eq!(NetError::ResolveFailed.to_string(), "ResolveFailed");
        assert_eq!(NetError::Host.to_string(), "Host");
        assert_eq!(
            TlsError::CertificateRejected.to_string(),
            "CertificateRejected"
        );
        assert_eq!(
            TlsError::Transport(NetError::ConnectionReset).to_string(),
            "Transport(ConnectionReset)"
        );
    }

    #[test]
    fn host_names_preserve_case_punycode_and_the_final_root_dot() {
        for name in [
            "localhost",
            "Example.COM",
            "xn--caf-dma.example",
            "example.com.",
        ] {
            let parsed = HostName::parse(name).unwrap();
            assert_eq!(parsed.as_bytes(), name.as_bytes());
            assert_eq!(parsed.to_string(), name);
        }
        assert_ne!(HostName::parse("a.test"), HostName::parse("A.test"));
    }

    #[test]
    fn host_names_reject_invalid_labels_without_normalization() {
        for name in [
            "", ".", ".a", "a..b", "a..", "-a", "a-", "a b", "a/b", "a\\b", "a\0b", "café", "a_b",
            "a:b",
        ] {
            assert_eq!(
                HostName::parse(name),
                Err(NetError::InvalidHostName),
                "{name:?}"
            );
        }
        assert_eq!(
            HostName::parse(&"a".repeat(64)),
            Err(NetError::InvalidHostName)
        );
        let valid = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61)
        );
        assert_eq!(valid.len(), 253);
        assert_eq!(
            HostName::parse(&valid).unwrap().as_bytes(),
            valid.as_bytes()
        );
        assert_eq!(
            HostName::parse(&(valid + ".")),
            Err(NetError::InvalidHostName)
        );
    }

    #[test]
    fn numeric_addresses_are_lookup_free_and_keep_the_address_family() {
        for literal in [
            "0.0.0.0",
            "255.255.255.255",
            "::",
            "::1",
            "2001:DB8::1",
            "::ffff:127.0.0.1",
        ] {
            let value = IpAddress::parse(literal).unwrap();
            assert_eq!(IpAddress::parse(&value.to_string()).unwrap(), value);
        }
        assert_ne!(
            IpAddress::parse("127.0.0.1"),
            IpAddress::parse("::ffff:127.0.0.1")
        );
        for invalid in [
            "example.com",
            "01.2.3.4",
            "256.0.0.1",
            "[::1]",
            "::1%eth0",
            "127.0.0.1:80",
            " ::1",
            "::1 ",
            "",
        ] {
            assert_eq!(IpAddress::parse(invalid), Err(NetError::InvalidAddress));
        }
    }

    #[test]
    fn zero_port_is_a_bind_value_and_never_a_destination() {
        let binding = address("127.0.0.1", 0);
        assert_eq!(binding.port(), 0);
        assert_eq!(binding.require_destination(), Err(NetError::InvalidPort));
        assert_eq!(
            address("::1", 65_535).require_destination().unwrap().port(),
            65_535
        );
        for invalid in [-1, 65_536, i128::MAX] {
            assert_eq!(
                SocketAddress::create(binding.ip(), invalid),
                Err(NetError::InvalidPort)
            );
        }
    }

    #[test]
    fn limits_separate_invalid_inputs_from_finite_target_resource_ceilings() {
        for invalid in [(0, 1, 1), (1, -1, 1), (1, 1, 0)] {
            assert_eq!(
                NetLimits::create(invalid.0, invalid.1, invalid.2),
                Err(NetError::InvalidLimit)
            );
        }
        for oversized in [
            (i128::MAX, 1, 1),
            (1, i128::MAX, 1),
            (1, 1, i128::MAX),
            (MAX_READ_BYTES as i128 + 1, 1, 1),
            (1, MAX_DATAGRAM_BYTES as i128 + 1, 1),
            (1, 1, MAX_RESOLVER_RESULTS as i128 + 1),
        ] {
            assert_eq!(
                NetLimits::create(oversized.0, oversized.1, oversized.2),
                Err(NetError::ResourceLimit)
            );
        }
        let defaults = NetLimits::default();
        assert_eq!(
            NetLimits::create(
                defaults.max_read() as i128,
                defaults.max_datagram() as i128,
                defaults.max_results() as i128
            )
            .unwrap(),
            defaults
        );
    }

    #[test]
    fn read_and_datagram_admission_refuse_oversized_requests() {
        let limits = NetLimits::create(4, 3, 1).unwrap();
        assert_eq!(limits.read_request(0), Err(NetError::InvalidLimit));
        assert_eq!(limits.read_request(-1), Err(NetError::InvalidLimit));
        assert_eq!(limits.read_request(4), Ok(4));
        assert_eq!(limits.read_request(5), Err(NetError::ResourceLimit));
        assert_eq!(limits.read_request(i128::MAX), Err(NetError::ResourceLimit));
        assert_eq!(limits.datagram_size(0), Ok(()));
        assert_eq!(limits.datagram_size(3), Ok(()));
        assert_eq!(limits.datagram_size(4), Err(NetError::DatagramTooLarge));
    }

    #[test]
    fn deadline_validation_and_outcome_priority_precede_registration() {
        let limits = NetLimits::default();
        assert_eq!(Deadline::create(0, 0), Err(NetError::InvalidDeadline));
        assert_eq!(
            Deadline::create(1, i128::MAX),
            Err(NetError::InvalidDeadline)
        );
        let deadline = Deadline::create(1, 5).unwrap();
        assert_eq!(
            NetOptions::create(Some(deadline), limits, 2),
            Err(NetError::InvalidDeadline)
        );
        let options = NetOptions::create(Some(deadline), limits, 1).unwrap();
        assert_eq!(
            options.before_commit(2, 5, true),
            Err(NetError::InvalidDeadline)
        );
        assert_eq!(
            options.before_commit(1, i128::MAX, true),
            Err(NetError::InvalidDeadline)
        );
        assert_eq!(options.before_commit(1, 5, true), Err(NetError::Cancelled));
        assert_eq!(options.before_commit(1, 5, false), Err(NetError::Timeout));
        assert_eq!(options.before_commit(1, 4, false), Ok(()));
        let before_origin =
            NetOptions::create(Some(Deadline::create(1, -1).unwrap()), limits, 1).unwrap();
        assert_eq!(
            before_origin.before_commit(1, 0, false),
            Err(NetError::Timeout)
        );
        let unbounded_time = NetOptions::create(None, limits, 0).unwrap();
        assert_eq!(unbounded_time.before_commit(0, i128::MAX, false), Ok(()));
        assert_eq!(
            unbounded_time.before_commit(0, 0, true),
            Err(NetError::Cancelled)
        );
    }

    #[test]
    fn resolver_deduplication_preserves_order_and_rejects_partial_publication() {
        let first = address("::1", 80);
        let second = address("127.0.0.1", 80);
        let limits = NetLimits::create(1, 1, 2).unwrap();
        assert_eq!(
            resolver_results(&[first, second, first], limits).unwrap(),
            vec![first, second]
        );
        assert_eq!(
            resolver_results(&[first, first], NetLimits::create(1, 1, 1).unwrap()).unwrap(),
            vec![first]
        );
        assert_eq!(
            resolver_results(&[first, second], NetLimits::create(1, 1, 1).unwrap()),
            Err(NetError::ResourceLimit)
        );
        assert_eq!(
            resolver_results(&[first, address("::1", 0)], limits),
            Err(NetError::InvalidPort)
        );
        assert_eq!(
            resolver_results(&vec![first; MAX_RESOLVER_RESULTS + 1], limits),
            Err(NetError::ResourceLimit)
        );
        assert_eq!(resolver_results(&[], limits).unwrap(), Vec::new());
    }
}
