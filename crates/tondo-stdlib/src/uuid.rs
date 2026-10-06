//! Fixed-width RFC 9562 UUID kernels with explicit generation inputs.
//!
//! This module performs no provider calls. v4 consumes a supplied 16-byte
//! entropy snapshot; v7 consumes checked Unix milliseconds and ten supplied
//! bytes. Provider capabilities, VM heap accounting and public Tondo/native
//! registration belong to later owner blocks.

use std::{cmp::Ordering, fmt};

use sha1::{Digest, Sha1};

/// Maximum accepted dashed UUID text including the optional URN prefix.
pub const MAX_TEXT_BYTES: usize = 45;
/// The default target limit on opaque UUIDv5 name bytes.
pub const DEFAULT_MAX_NAME_BYTES: usize = 16 * 1024 * 1024;
/// Largest Unix millisecond timestamp representable by UUIDv7.
pub const MAX_UNIX_MILLISECONDS: i128 = (1_i128 << 48) - 1;
const HEX: &[u8; 16] = b"0123456789abcdef";
const URN_PREFIX: &[u8; 9] = b"urn:uuid:";

/// UUID variant bits; external values in every variant are preserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidVariant {
    Rfc9562,
    Ncs,
    Microsoft,
    Future,
}

/// Closed nominal errors shared with the future capability/provider boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UuidErrorKind {
    InvalidTextLength,
    InvalidCharacter,
    InvalidSeparator,
    InvalidUrnPrefix,
    InvalidBytesLength,
    NameLimitExceeded,
    TimestampOutOfRange,
    EntropyUnavailable,
    EntropyFailure,
    ClockUnavailable,
    ClockFailure,
    ProviderMisconfigured,
    ResourceLimit,
    OutOfMemory,
}

/// A nominal error with an absolute input byte offset for lexical failures.
/// Length, limit, timestamp and provider failures have no lexical offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UuidError {
    pub kind: UuidErrorKind,
    pub offset: Option<usize>,
}

impl UuidError {
    pub const fn new(kind: UuidErrorKind, offset: Option<usize>) -> Self {
        Self { kind, offset }
    }
}

impl fmt::Display for UuidError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self.kind)?;
        if let Some(offset) = self.offset {
            write!(formatter, " at byte {offset}")?;
        }
        Ok(())
    }
}

impl std::error::Error for UuidError {}

/// Rust-kernel target configuration; zero permits only an empty v5 name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UuidLimits {
    pub max_name_bytes: usize,
}

impl Default for UuidLimits {
    fn default() -> Self {
        Self {
            max_name_bytes: DEFAULT_MAX_NAME_BYTES,
        }
    }
}

/// An immutable value containing exactly sixteen bytes in network order.
/// Equality, hashing and ordering use those bytes, with no host identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Uuid([u8; 16]);

impl Uuid {
    pub const fn nil() -> Self {
        Self([0; 16])
    }

    pub const fn max() -> Self {
        Self([255; 16])
    }

    /// Copy exactly sixteen bytes; no native-endian or COM GUID conversion.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, UuidError> {
        let bytes = <[u8; 16]>::try_from(bytes)
            .map_err(|_| UuidError::new(UuidErrorKind::InvalidBytesLength, None))?;
        Ok(Self(bytes))
    }

    /// Return a fresh fixed-width copy, without allocating a Rust heap buffer.
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// Parse exactly 36 dashed ASCII bytes or 45 bytes with a case-insensitive
    /// `urn:uuid:` prefix. All version nibbles and variants are accepted.
    pub fn parse(text: &str) -> Result<Self, UuidError> {
        let input = text.as_bytes();
        let start = match input.len() {
            36 => 0,
            MAX_TEXT_BYTES => {
                for (offset, expected) in URN_PREFIX.iter().enumerate() {
                    if !input[offset].eq_ignore_ascii_case(expected) {
                        return Err(UuidError::new(
                            UuidErrorKind::InvalidUrnPrefix,
                            Some(offset),
                        ));
                    }
                }
                URN_PREFIX.len()
            }
            _ => return Err(UuidError::new(UuidErrorKind::InvalidTextLength, None)),
        };
        let mut bytes = [0; 16];
        let mut nibble = 0;
        for (index, byte) in input[start..].iter().copied().enumerate() {
            let offset = start + index;
            if matches!(index, 8 | 13 | 18 | 23) {
                if byte != b'-' {
                    return Err(UuidError::new(
                        UuidErrorKind::InvalidSeparator,
                        Some(offset),
                    ));
                }
            } else {
                let value = match byte {
                    b'0'..=b'9' => byte - b'0',
                    b'a'..=b'f' => byte - b'a' + 10,
                    b'A'..=b'F' => byte - b'A' + 10,
                    _ => {
                        return Err(UuidError::new(
                            UuidErrorKind::InvalidCharacter,
                            Some(offset),
                        ));
                    }
                };
                bytes[nibble / 2] |= value << (4 * (1 - nibble % 2));
                nibble += 1;
            }
        }
        Ok(Self(bytes))
    }

    /// Materialize canonical lowercase dashed text with recoverable reserve
    /// failure. Fixed-width formatting itself uses stack storage only.
    pub fn try_to_string(self) -> Result<String, UuidError> {
        let mut text = String::new();
        text.try_reserve_exact(36)
            .map_err(|_| UuidError::new(UuidErrorKind::OutOfMemory, None))?;
        for byte in self.text_bytes() {
            text.push(char::from(byte));
        }
        Ok(text)
    }

    pub const fn version(self) -> u8 {
        self.0[6] >> 4
    }

    pub const fn variant(self) -> UuidVariant {
        match self.0[8] {
            0..=127 => UuidVariant::Ncs,
            128..=191 => UuidVariant::Rfc9562,
            192..=223 => UuidVariant::Microsoft,
            _ => UuidVariant::Future,
        }
    }

    pub fn is_nil(self) -> bool {
        self == Self::nil()
    }

    pub fn is_max(self) -> bool {
        self == Self::max()
    }

    /// Unsigned lexicographic network-byte order, independent of version/time.
    pub fn compare(self, other: Self) -> i32 {
        match self.cmp(&other) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }

    /// Construct v4 from exactly sixteen supplied bytes; no provider or retry.
    pub fn v4(entropy: &[u8]) -> Result<Self, UuidError> {
        let bytes = <[u8; 16]>::try_from(entropy)
            .map_err(|_| UuidError::new(UuidErrorKind::ProviderMisconfigured, None))?;
        Ok(Self::generated(bytes, 4))
    }

    /// Hash namespace network bytes followed by the opaque name, without
    /// allocating their concatenation or normalizing the name. The caller
    /// supplies the target's finite limit; it is checked before hashing.
    pub fn v5(namespace: Self, name: &[u8], limits: UuidLimits) -> Result<Self, UuidError> {
        if name.len() > limits.max_name_bytes {
            return Err(UuidError::new(UuidErrorKind::NameLimitExceeded, None));
        }
        let mut hasher = Sha1::new();
        hasher.update(namespace.0);
        hasher.update(name);
        let digest = hasher.finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest[..16]);
        Ok(Self::generated(bytes, 5))
    }

    /// Construct v7 from checked UTC Unix milliseconds and exactly ten
    /// supplied entropy bytes. Range errors precede entropy-length errors.
    /// No counter, strict monotonicity or uniqueness guarantee is added.
    pub fn v7(unix_milliseconds: i128, entropy: &[u8]) -> Result<Self, UuidError> {
        if !(0..=MAX_UNIX_MILLISECONDS).contains(&unix_milliseconds) {
            return Err(UuidError::new(UuidErrorKind::TimestampOutOfRange, None));
        }
        if entropy.len() != 10 {
            return Err(UuidError::new(UuidErrorKind::ProviderMisconfigured, None));
        }
        let mut bytes = [0; 16];
        bytes[..6].copy_from_slice(&unix_milliseconds.to_be_bytes()[10..]);
        bytes[6..].copy_from_slice(entropy);
        Ok(Self::generated(bytes, 7))
    }

    fn generated(mut bytes: [u8; 16], version: u8) -> Self {
        bytes[6] = (bytes[6] & 0x0f) | (version << 4);
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(bytes)
    }

    fn text_bytes(self) -> [u8; 36] {
        let mut text = [b'-'; 36];
        let mut offset = 0;
        for byte in self.0 {
            if matches!(offset, 8 | 13 | 18 | 23) {
                offset += 1;
            }
            text[offset] = HEX[usize::from(byte >> 4)];
            text[offset + 1] = HEX[usize::from(byte & 0x0f)];
            offset += 2;
        }
        text
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        for byte in self.text_bytes() {
            formatter.write_char(char::from(byte))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap, HashSet};

    use super::{MAX_UNIX_MILLISECONDS, Uuid, UuidError, UuidErrorKind, UuidLimits, UuidVariant};

    fn error(kind: UuidErrorKind, offset: Option<usize>) -> UuidError {
        UuidError::new(kind, offset)
    }

    #[test]
    fn representation_copies_network_bytes_and_supports_keys() {
        fn traits<T: Copy + Send + Sync + Ord + std::hash::Hash>() {}
        traits::<Uuid>();
        assert_eq!(std::mem::size_of::<Uuid>(), 16);
        let mut input = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let uuid = Uuid::from_bytes(&input).unwrap();
        let original = input;
        input.fill(255);
        let mut output = uuid.to_bytes();
        output.fill(0);
        assert_eq!(uuid.to_bytes(), original);
        assert_eq!(
            uuid.try_to_string().unwrap(),
            "00010203-0405-0607-0809-0a0b0c0d0e0f"
        );
        assert_eq!(HashSet::from([uuid, uuid]).len(), 1);
        assert_eq!(HashMap::from([(uuid, 42)]).get(&uuid), Some(&42));
    }

    #[test]
    fn sentinels_round_trip_and_preserve_special_variants() {
        for (uuid, text, variant, version) in [
            (
                Uuid::nil(),
                "00000000-0000-0000-0000-000000000000",
                UuidVariant::Ncs,
                0,
            ),
            (
                Uuid::max(),
                "ffffffff-ffff-ffff-ffff-ffffffffffff",
                UuidVariant::Future,
                15,
            ),
        ] {
            assert_eq!(uuid.try_to_string().unwrap(), text);
            assert_eq!(uuid.to_string(), text);
            assert_eq!(Uuid::parse(text), Ok(uuid));
            assert_eq!(uuid.variant(), variant);
            assert_eq!(uuid.version(), version);
        }
        assert!(Uuid::nil().is_nil());
        assert!(!Uuid::max().is_nil());
        assert!(Uuid::max().is_max());
        assert!(!Uuid::nil().is_max());
    }

    #[test]
    fn parse_accepts_dashed_mixed_case_and_urn() {
        let expected = Uuid::from_bytes(&[
            0x91, 0x91, 8, 0xf7, 0x52, 0xd1, 0x43, 0x20, 0x9b, 0xac, 0xf8, 0x47, 0xdb, 0x41, 0x48,
            0xa8,
        ])
        .unwrap();
        for text in [
            "919108f7-52d1-4320-9bac-f847db4148a8",
            "919108F7-52D1-4320-9bAC-F847DB4148A8",
            "URN:uUiD:919108F7-52D1-4320-9BAC-F847DB4148A8",
        ] {
            assert_eq!(Uuid::parse(text), Ok(expected));
        }
    }

    #[test]
    fn parse_rejects_lengths_before_lexical_scanning() {
        for length in (0..=64).filter(|length| !matches!(length, 36 | 45)) {
            assert_eq!(
                Uuid::parse(&"0".repeat(length)),
                Err(error(UuidErrorKind::InvalidTextLength, None))
            );
        }
        for text in [
            "{00000000-0000-0000-0000-000000000000}",
            "00000000000000000000000000000000",
            " 00000000-0000-0000-0000-000000000000",
            "00000000-0000-0000-0000-000000000000\n",
        ] {
            assert_eq!(
                Uuid::parse(text),
                Err(error(UuidErrorKind::InvalidTextLength, None))
            );
        }
    }

    #[test]
    fn parse_reports_first_absolute_character_separator_and_prefix_offsets() {
        for prefix in ["", "urn:uuid:"] {
            let valid = format!("{prefix}00000000-0000-0000-0000-000000000000");
            for index in 0..36 {
                let offset = prefix.len() + index;
                let mut invalid = valid.as_bytes().to_vec();
                invalid[offset] = b'g';
                let kind = if matches!(index, 8 | 13 | 18 | 23) {
                    UuidErrorKind::InvalidSeparator
                } else {
                    UuidErrorKind::InvalidCharacter
                };
                assert_eq!(
                    Uuid::parse(std::str::from_utf8(&invalid).unwrap()),
                    Err(error(kind, Some(offset)))
                );
            }
        }
        for offset in 0..9 {
            let mut invalid = b"urn:uuid:00000000-0000-0000-0000-000000000000".to_vec();
            invalid[offset] = b'x';
            assert_eq!(
                Uuid::parse(std::str::from_utf8(&invalid).unwrap()),
                Err(error(UuidErrorKind::InvalidUrnPrefix, Some(offset)))
            );
        }
        assert_eq!(
            Uuid::parse("é000000-0000-0000-0000-000000000000"),
            Err(error(UuidErrorKind::InvalidCharacter, Some(0)))
        );
        assert_eq!(
            Uuid::parse("g0000000-0000-0000-x000-000000000000"),
            Err(error(UuidErrorKind::InvalidCharacter, Some(0)))
        );
    }

    #[test]
    fn byte_inputs_require_exact_length_without_publishing_partial_values() {
        for length in (0..=32).filter(|length| *length != 16) {
            assert_eq!(
                Uuid::from_bytes(&vec![0; length]),
                Err(error(UuidErrorKind::InvalidBytesLength, None))
            );
        }
    }

    #[test]
    fn every_variant_byte_and_version_nibble_is_preserved() {
        for variant_byte in 0_u8..=255 {
            for version in 0..16 {
                let mut bytes = [0; 16];
                bytes[6] = version << 4 | 15;
                bytes[8] = variant_byte;
                let uuid = Uuid::from_bytes(&bytes).unwrap();
                let expected = match variant_byte {
                    0..=127 => UuidVariant::Ncs,
                    128..=191 => UuidVariant::Rfc9562,
                    192..=223 => UuidVariant::Microsoft,
                    _ => UuidVariant::Future,
                };
                assert_eq!(uuid.version(), version);
                assert_eq!(uuid.variant(), expected);
                assert_eq!(
                    Uuid::parse(&uuid.try_to_string().unwrap())
                        .unwrap()
                        .to_bytes(),
                    bytes
                );
            }
        }
    }

    #[test]
    fn ordering_is_unsigned_lexicographic_for_every_byte_position() {
        for position in 0..16 {
            let mut lower = [255; 16];
            let mut higher = [0; 16];
            lower[..position].fill(0);
            lower[position] = 127;
            higher[position] = 128;
            let lower = Uuid::from_bytes(&lower).unwrap();
            let higher = Uuid::from_bytes(&higher).unwrap();
            assert_eq!(lower.compare(higher), -1);
            assert_eq!(higher.compare(lower), 1);
            assert_eq!(lower.compare(lower), 0);
            assert_eq!(
                BTreeSet::from([higher, lower])
                    .into_iter()
                    .collect::<Vec<_>>(),
                [lower, higher]
            );
        }
    }

    #[test]
    fn v4_matches_rfc_9562_vector() {
        let entropy = [
            0x91, 0x91, 8, 0xf7, 0x52, 0xd1, 0x33, 0x20, 0x5b, 0xac, 0xf8, 0x47, 0xdb, 0x41, 0x48,
            0xa8,
        ];
        assert_eq!(
            Uuid::v4(&entropy).unwrap().try_to_string().unwrap(),
            "919108f7-52d1-4320-9bac-f847db4148a8"
        );
    }

    #[test]
    fn v4_preserves_all_122_entropy_bits_and_only_replaces_six_layout_bits() {
        for bit in 0..128 {
            let mut entropy = [0; 16];
            entropy[bit / 8] |= 1 << (7 - bit % 8);
            let result = Uuid::v4(&entropy).unwrap();
            let mut expected = entropy;
            expected[6] = entropy[6] & 15 | 0x40;
            expected[8] = entropy[8] & 63 | 0x80;
            assert_eq!(result.to_bytes(), expected);
            assert_eq!(result.version(), 4);
            assert_eq!(result.variant(), UuidVariant::Rfc9562);
        }
    }

    #[test]
    fn v5_matches_rfc_9562_vector_and_hashes_opaque_name_bytes() {
        let namespace = Uuid::parse("6ba7b810-9dad-11d1-80b4-00c04fd430c8").unwrap();
        assert_eq!(
            Uuid::v5(namespace, b"www.example.com", UuidLimits::default())
                .unwrap()
                .try_to_string()
                .unwrap(),
            "2ed6657d-e927-568b-95e1-2665a8aea6a2"
        );
        assert_ne!(
            Uuid::v5(namespace, "é".as_bytes(), UuidLimits::default()),
            Uuid::v5(namespace, "e\u{301}".as_bytes(), UuidLimits::default())
        );
        assert_ne!(
            Uuid::v5(namespace, &[0xff, 0], UuidLimits::default()),
            Uuid::v5(Uuid::nil(), &[0xff, 0], UuidLimits::default())
        );
    }

    #[test]
    fn v5_checks_exact_and_zero_name_limits_before_hashing() {
        let limits = UuidLimits { max_name_bytes: 3 };
        assert!(Uuid::v5(Uuid::nil(), b"abc", limits).is_ok());
        assert_eq!(
            Uuid::v5(Uuid::nil(), b"abcd", limits),
            Err(error(UuidErrorKind::NameLimitExceeded, None))
        );
        let limits = UuidLimits { max_name_bytes: 0 };
        assert!(Uuid::v5(Uuid::nil(), b"", limits).is_ok());
        assert_eq!(
            Uuid::v5(Uuid::nil(), b"x", limits),
            Err(error(UuidErrorKind::NameLimitExceeded, None))
        );
        let name = vec![42; super::DEFAULT_MAX_NAME_BYTES];
        assert!(Uuid::v5(Uuid::max(), &name, UuidLimits::default()).is_ok());
        let name = vec![42; super::DEFAULT_MAX_NAME_BYTES + 1];
        assert_eq!(
            Uuid::v5(Uuid::max(), &name, UuidLimits::default()),
            Err(error(UuidErrorKind::NameLimitExceeded, None))
        );
    }

    #[test]
    fn v7_matches_rfc_9562_vector() {
        let entropy = [0x0c, 0xc3, 0x18, 0xc4, 0xdc, 0x0c, 0x0c, 7, 0x39, 0x8f];
        assert_eq!(
            Uuid::v7(1_645_557_742_000, &entropy)
                .unwrap()
                .try_to_string()
                .unwrap(),
            "017f22e2-79b0-7cc3-98c4-dc0c0c07398f"
        );
    }

    #[test]
    fn v7_timestamp_boundaries_are_checked_without_wrap_or_saturation() {
        assert_eq!(
            Uuid::v7(0, &[0; 10]).unwrap().try_to_string().unwrap(),
            "00000000-0000-7000-8000-000000000000"
        );
        assert_eq!(
            Uuid::v7(MAX_UNIX_MILLISECONDS, &[255; 10])
                .unwrap()
                .try_to_string()
                .unwrap(),
            "ffffffff-ffff-7fff-bfff-ffffffffffff"
        );
        for timestamp in [i128::MIN, -1, MAX_UNIX_MILLISECONDS + 1, i128::MAX] {
            assert_eq!(
                Uuid::v7(timestamp, &[]),
                Err(error(UuidErrorKind::TimestampOutOfRange, None))
            );
        }
    }

    #[test]
    fn v7_preserves_all_74_entropy_bits_with_no_monotonic_state() {
        for bit in 0..80 {
            let mut entropy = [0; 10];
            entropy[bit / 8] |= 1 << (7 - bit % 8);
            let result = Uuid::v7(1, &entropy).unwrap();
            let bytes = result.to_bytes();
            let mut expected = entropy;
            expected[0] = entropy[0] & 15 | 0x70;
            expected[2] = entropy[2] & 63 | 0x80;
            assert_eq!(bytes[..6], [0, 0, 0, 0, 0, 1]);
            assert_eq!(bytes[6..], expected);
            assert_eq!(result.version(), 7);
            assert_eq!(result.variant(), UuidVariant::Rfc9562);
        }
        assert!(Uuid::v7(1, &[255; 10]).unwrap() > Uuid::v7(1, &[0; 10]).unwrap());
        assert!(Uuid::v7(1, &[255; 10]).unwrap() < Uuid::v7(2, &[0; 10]).unwrap());
    }

    #[test]
    fn generation_rejects_incomplete_or_excess_entropy_and_recovers_statelessly() {
        for length in (0..=32).filter(|length| *length != 16) {
            assert_eq!(
                Uuid::v4(&vec![0; length]),
                Err(error(UuidErrorKind::ProviderMisconfigured, None))
            );
        }
        for length in (0..=20).filter(|length| *length != 10) {
            assert_eq!(
                Uuid::v7(0, &vec![0; length]),
                Err(error(UuidErrorKind::ProviderMisconfigured, None))
            );
        }
        let first = Uuid::v4(&[42; 16]).unwrap();
        assert_eq!(Uuid::v4(&[42; 16]), Ok(first));
        assert_eq!(Uuid::v7(42, &[42; 10]), Uuid::v7(42, &[42; 10]));
        assert_eq!(
            Uuid::v5(Uuid::nil(), b"name", UuidLimits::default()),
            Uuid::v5(Uuid::nil(), b"name", UuidLimits::default())
        );
    }

    #[test]
    fn immutable_values_are_shared_across_threads_without_provider_state() {
        let uuid = Uuid::v5(Uuid::nil(), b"parallel", UuidLimits::default()).unwrap();
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        for _ in 0..64 {
                            assert_eq!(Uuid::parse(&uuid.try_to_string().unwrap()), Ok(uuid));
                            assert_eq!(
                                Uuid::v5(Uuid::nil(), b"parallel", UuidLimits::default()),
                                Ok(uuid)
                            );
                        }
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        });
    }

    #[test]
    fn nominal_errors_have_stable_environment_free_display() {
        for (kind, name) in [
            (UuidErrorKind::InvalidTextLength, "InvalidTextLength"),
            (UuidErrorKind::InvalidCharacter, "InvalidCharacter"),
            (UuidErrorKind::InvalidSeparator, "InvalidSeparator"),
            (UuidErrorKind::InvalidUrnPrefix, "InvalidUrnPrefix"),
            (UuidErrorKind::InvalidBytesLength, "InvalidBytesLength"),
            (UuidErrorKind::NameLimitExceeded, "NameLimitExceeded"),
            (UuidErrorKind::TimestampOutOfRange, "TimestampOutOfRange"),
            (UuidErrorKind::EntropyUnavailable, "EntropyUnavailable"),
            (UuidErrorKind::EntropyFailure, "EntropyFailure"),
            (UuidErrorKind::ClockUnavailable, "ClockUnavailable"),
            (UuidErrorKind::ClockFailure, "ClockFailure"),
            (
                UuidErrorKind::ProviderMisconfigured,
                "ProviderMisconfigured",
            ),
            (UuidErrorKind::ResourceLimit, "ResourceLimit"),
            (UuidErrorKind::OutOfMemory, "OutOfMemory"),
        ] {
            assert_eq!(error(kind, None).to_string(), name);
            assert_eq!(
                error(kind, Some(9)).to_string(),
                format!("{name} at byte 9")
            );
        }
    }
}
