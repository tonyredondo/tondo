//! Independent finite oracle for networking values and admission.
//! No production imports, sockets, clocks, DNS parser, or TLS record parser.

use std::collections::BTreeSet;

pub const MAX_REFERENCE_BYTES: usize = 128;
pub const MAX_REFERENCE_ADDRESSES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OutsideDomain,
    InvalidHostName,
    InvalidPort,
    InvalidLimit,
    InvalidDeadline,
    ResourceLimit,
    DatagramTooLarge,
    Cancelled,
    Timeout,
    Host,
}

/// A scanner independent of production's label splitting. Retain spelling.
pub fn host_name(text: &str) -> Result<String, Error> {
    let bytes = text.as_bytes();
    if bytes.is_empty() || bytes.len() > 253 {
        return Err(Error::InvalidHostName);
    }
    let mut label_length = 0;
    let mut last_alphanumeric = false;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => {
                label_length += 1;
                last_alphanumeric = true;
            }
            b'-' if label_length != 0 => {
                label_length += 1;
                last_alphanumeric = false;
            }
            b'.' if label_length != 0 && last_alphanumeric => {
                if index + 1 == bytes.len() {
                    return Ok(text.to_owned());
                }
                label_length = 0;
                last_alphanumeric = false;
            }
            _ => return Err(Error::InvalidHostName),
        }
        if label_length > 63 {
            return Err(Error::InvalidHostName);
        }
    }
    if last_alphanumeric {
        Ok(text.to_owned())
    } else {
        Err(Error::InvalidHostName)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Address {
    V4([u8; 4], u16),
    V6([u8; 16], u16),
}

impl Address {
    pub fn ipv4(octets: [u8; 4], port: i128) -> Result<Self, Error> {
        if !(0..=65535).contains(&port) {
            return Err(Error::InvalidPort);
        }
        Ok(Self::V4(octets, port as u16))
    }

    pub fn port(self) -> u16 {
        match self {
            Self::V4(_, port) | Self::V6(_, port) => port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub read: usize,
    pub datagram: usize,
    pub results: usize,
}

impl Limits {
    pub fn create(read: i128, datagram: i128, results: i128) -> Result<Self, Error> {
        if [read, datagram, results]
            .into_iter()
            .any(|value| value <= 0)
        {
            return Err(Error::InvalidLimit);
        }
        if read > 67_108_864 || datagram > 65_535 || results > 1_024 {
            return Err(Error::ResourceLimit);
        }
        Ok(Self {
            read: read as usize,
            datagram: datagram as usize,
            results: results as usize,
        })
    }

    pub fn request(self, count: i128) -> Result<usize, Error> {
        if count <= 0 {
            Err(Error::InvalidLimit)
        } else if count > self.read as i128 {
            Err(Error::ResourceLimit)
        } else {
            Ok(count as usize)
        }
    }
}

/// Provider domains and ticks are explicit; the oracle never reads a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    pub domain: u64,
    pub tick: i64,
}

impl Deadline {
    pub fn create(domain: u64, tick: i128) -> Result<Self, Error> {
        if domain == 0 || tick < i64::MIN as i128 || tick > i64::MAX as i128 {
            Err(Error::InvalidDeadline)
        } else {
            Ok(Self {
                domain,
                tick: tick as i64,
            })
        }
    }
}

pub fn before_commit(
    deadline: Option<Deadline>,
    active_domain: u64,
    now: i128,
    cancelled: bool,
) -> Result<(), Error> {
    if let Some(deadline) = deadline
        && (deadline.domain != active_domain || now < i64::MIN as i128 || now > i64::MAX as i128)
    {
        return Err(Error::InvalidDeadline);
    }
    if cancelled {
        return Err(Error::Cancelled);
    }
    if deadline.is_some_and(|deadline| now >= deadline.tick as i128) {
        return Err(Error::Timeout);
    }
    Ok(())
}

/// Ordered set insertion checks uniqueness before the caller's result limit.
/// The finite oracle cannot establish a rejection beyond its retained domain.
pub fn resolve(addresses: &[Address], limits: Limits) -> Result<Vec<Address>, Error> {
    if addresses.len() > MAX_REFERENCE_ADDRESSES {
        return Err(Error::OutsideDomain);
    }
    let mut identities = BTreeSet::new();
    let mut ordered = Vec::new();
    for address in addresses.iter().copied() {
        if address.port() == 0 {
            return Err(Error::InvalidPort);
        }
        if identities.insert(address) {
            if identities.len() > limits.results {
                return Err(Error::ResourceLimit);
            }
            ordered.push(address);
        }
    }
    Ok(ordered)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    Data(Vec<u8>),
    Eof,
}

pub fn read(bytes: &[u8], requested: usize, eof: bool) -> Result<Read, Error> {
    if bytes.len() > MAX_REFERENCE_BYTES {
        return Err(Error::OutsideDomain);
    }
    match (requested, bytes.len(), eof) {
        (0, _, _) => Err(Error::Host),
        (_, length, _) if length > requested => Err(Error::Host),
        (_, 0, true) => Ok(Read::Eof),
        (_, 0, false) => Err(Error::Host),
        _ => Ok(Read::Data(bytes.to_vec())),
    }
}

pub fn write(requested: usize, accepted: usize) -> Result<usize, Error> {
    match (requested, accepted) {
        (0, 0) => Ok(0),
        (_, 0) => Err(Error::Host),
        (requested, accepted) if accepted <= requested => Ok(accepted),
        _ => Err(Error::Host),
    }
}

pub fn send_datagram(requested: usize, accepted: usize) -> Result<(), Error> {
    if requested == accepted {
        Ok(())
    } else {
        Err(Error::Host)
    }
}

pub fn receive_datagram(
    bytes: &[u8],
    source: Address,
    wire_length: usize,
    limits: Limits,
) -> Result<(Vec<u8>, Address), Error> {
    if bytes.len() > MAX_REFERENCE_BYTES {
        return Err(Error::OutsideDomain);
    }
    if wire_length > limits.datagram {
        return Err(Error::DatagramTooLarge);
    }
    if source.port() == 0 || bytes.len() != wire_length {
        return Err(Error::Host);
    }
    Ok((bytes.to_vec(), source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_retains_case_and_rejects_incomplete_labels() {
        for text in ["example", "EXample.test.", "a-b.test", "xn--test.test"] {
            assert_eq!(host_name(text), Ok(text.to_owned()));
        }
        for text in [
            "", ".", ".a", "a..", "-a", "a-", "a-.b", "a/b", "é.test", "a\0b",
        ] {
            assert_eq!(host_name(text), Err(Error::InvalidHostName));
        }
        assert!(host_name(&"a".repeat(63)).is_ok());
        assert_eq!(host_name(&"a".repeat(64)), Err(Error::InvalidHostName));
        let maximum = format!(
            "{}.{}.{}.{}",
            "a".repeat(63),
            "b".repeat(63),
            "c".repeat(63),
            "d".repeat(61)
        );
        assert_eq!(maximum.len(), 253);
        assert!(host_name(&maximum).is_ok());
        assert_eq!(host_name(&(maximum + ".")), Err(Error::InvalidHostName));
    }

    #[test]
    fn deadline_failure_priority_is_explicit_and_unbounded_now_needs_no_clock() {
        let deadline = Some(Deadline::create(7, 10).unwrap());
        assert_eq!(
            before_commit(deadline, 8, 10, true),
            Err(Error::InvalidDeadline)
        );
        assert_eq!(before_commit(deadline, 7, 10, true), Err(Error::Cancelled));
        assert_eq!(before_commit(deadline, 7, 10, false), Err(Error::Timeout));
        assert_eq!(before_commit(deadline, 7, 9, false), Ok(()));
        assert_eq!(before_commit(None, 0, i128::MAX, false), Ok(()));
    }

    #[test]
    fn resolver_is_atomic_and_preserves_family_identity() {
        let limits = Limits::create(8, 8, 2).unwrap();
        let a = Address::ipv4([192, 0, 2, 2], 443).unwrap();
        let b = Address::ipv4([192, 0, 2, 1], 443).unwrap();
        let mut mapped = [0; 16];
        mapped[10..].copy_from_slice(&[255, 255, 192, 0, 2, 2]);
        let c = Address::V6(mapped, 443);
        assert_eq!(resolve(&[a, b, a], limits), Ok(vec![a, b]));
        assert_eq!(resolve(&[a, c], limits), Ok(vec![a, c]));
        assert_eq!(resolve(&[a, b, c], limits), Err(Error::ResourceLimit));
        assert_eq!(
            resolve(&[Address::ipv4([127, 0, 0, 1], 0).unwrap()], limits),
            Err(Error::InvalidPort)
        );
        assert_eq!(
            resolve(&[a; MAX_REFERENCE_ADDRESSES + 1], limits),
            Err(Error::OutsideDomain)
        );
    }

    #[test]
    fn final_tcp_chunk_precedes_eof_and_datagrams_never_truncate() {
        let limits = Limits::create(8, 8, 2).unwrap();
        let source = Address::ipv4([127, 0, 0, 1], 42).unwrap();
        assert_eq!(read(b"end", 3, true), Ok(Read::Data(b"end".to_vec())));
        assert_eq!(read(b"", 3, true), Ok(Read::Eof));
        assert_eq!(read(b"", 3, false), Err(Error::Host));
        assert_eq!(
            receive_datagram(b"", source, 0, limits),
            Ok((vec![], source))
        );
        assert_eq!(receive_datagram(b"ab", source, 3, limits), Err(Error::Host));
        assert_eq!(
            receive_datagram(b"ab", source, 9, limits),
            Err(Error::DatagramTooLarge)
        );
        assert_eq!(send_datagram(3, 2), Err(Error::Host));
        assert_eq!(write(3, 2), Ok(2));
        assert_eq!(write(3, 0), Err(Error::Host));
    }
}
