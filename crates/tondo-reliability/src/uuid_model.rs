//! Independent finite-domain UUID values, generation and provider transcripts.
//!
//! The reference uses a single integer and its own bounded UUIDv5 computation.
//! OutsideDomain is a reference bound, never a production rejection.

use std::fmt;

pub const MAX_REFERENCE_NAME_BYTES: usize = 96;
pub const MAX_REFERENCE_PROVIDER_ROWS: usize = 16;
pub const MAX_UUID_FUZZ_INPUT_BYTES: usize = 4096;
pub const MAX_UUID_FUZZ_STEPS: usize = 512;
pub const MAX_MILLISECONDS: i128 = 281_474_976_710_655;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
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
    OutsideDomain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceError {
    pub kind: ReferenceErrorKind,
    pub offset: Option<usize>,
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}", self.kind)?;
        if let Some(offset) = self.offset {
            write!(formatter, " at byte {offset}")?;
        }
        Ok(())
    }
}

fn error(kind: ReferenceErrorKind) -> ReferenceError {
    ReferenceError { kind, offset: None }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReferenceUuid(u128);

impl ReferenceUuid {
    pub const fn nil() -> Self {
        Self(0)
    }

    pub const fn max() -> Self {
        Self(u128::MAX)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReferenceError> {
        if bytes.len() != 16 {
            return Err(error(ReferenceErrorKind::InvalidBytesLength));
        }
        Ok(Self(
            bytes
                .iter()
                .fold(0, |value, byte| (value << 8) | u128::from(*byte)),
        ))
    }

    pub const fn to_bytes(self) -> [u8; 16] {
        self.0.to_be_bytes()
    }

    pub fn canonical(self) -> String {
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            self.0 >> 96,
            (self.0 >> 80) & 0xffff,
            (self.0 >> 64) & 0xffff,
            (self.0 >> 48) & 0xffff,
            self.0 & 0xffff_ffff_ffff
        )
    }

    pub fn parse(text: &str) -> Result<Self, ReferenceError> {
        let bytes = text.as_bytes();
        let prefix = match bytes.len() {
            36 => 0,
            45 => {
                for (offset, (actual, expected)) in bytes.iter().zip(b"urn:uuid:").enumerate() {
                    if actual.to_ascii_lowercase() != *expected {
                        return Err(ReferenceError {
                            kind: ReferenceErrorKind::InvalidUrnPrefix,
                            offset: Some(offset),
                        });
                    }
                }
                9
            }
            _ => return Err(error(ReferenceErrorKind::InvalidTextLength)),
        };
        let mut value = 0;
        for (index, byte) in bytes[prefix..].iter().enumerate() {
            let kind = if [8, 13, 18, 23].contains(&index) {
                if *byte == b'-' {
                    continue;
                }
                ReferenceErrorKind::InvalidSeparator
            } else if let Some(digit) = char::from(*byte).to_digit(16) {
                value = (value << 4) | u128::from(digit);
                continue;
            } else {
                ReferenceErrorKind::InvalidCharacter
            };
            return Err(ReferenceError {
                kind,
                offset: Some(prefix + index),
            });
        }
        Ok(Self(value))
    }

    pub const fn version(self) -> u8 {
        ((self.0 >> 76) & 15) as u8
    }

    pub const fn variant(self) -> &'static str {
        if self.0 & (1 << 63) == 0 {
            "Ncs"
        } else if self.0 & (1 << 62) == 0 {
            "Rfc9562"
        } else if self.0 & (1 << 61) == 0 {
            "Microsoft"
        } else {
            "Future"
        }
    }

    pub fn compare(self, other: Self) -> i32 {
        i32::from(self.0 > other.0) - i32::from(self.0 < other.0)
    }

    pub fn v4(entropy: &[u8]) -> Result<Self, ReferenceError> {
        Self::from_bytes(entropy)
            .map(|value| value.with_layout(4))
            .map_err(|_| error(ReferenceErrorKind::ProviderMisconfigured))
    }

    pub fn v5(self, name: &[u8], limit: usize) -> Result<Self, ReferenceError> {
        if name.len() > MAX_REFERENCE_NAME_BYTES {
            return Err(error(ReferenceErrorKind::OutsideDomain));
        }
        if name.len() > limit {
            return Err(error(ReferenceErrorKind::NameLimitExceeded));
        }
        Ok(Self(bounded_name_digest(self.0, name)).with_layout(5))
    }

    pub fn v7(milliseconds: i128, entropy: &[u8]) -> Result<Self, ReferenceError> {
        if !(0..=MAX_MILLISECONDS).contains(&milliseconds) {
            return Err(error(ReferenceErrorKind::TimestampOutOfRange));
        }
        if entropy.len() != 10 {
            return Err(error(ReferenceErrorKind::ProviderMisconfigured));
        }
        let low = entropy
            .iter()
            .fold(0, |value, byte| (value << 8) | u128::from(*byte));
        Ok(Self(((milliseconds as u128) << 80) | low).with_layout(7))
    }

    fn with_layout(self, version: u128) -> Self {
        let fixed = (15 << 76) | (3 << 62);
        Self((self.0 & !fixed) | (version << 76) | (2 << 62))
    }
}

// FIPS 180-4 section 6.1.2, bounded to namespace + at most 96 name bytes.
// The whole padded message and the expanded schedule are independent of the
// production dependency's streaming implementation. Only its high 128 bits
// are needed for UUIDv5; this is not a general-purpose hashing API.
fn bounded_name_digest(namespace: u128, name: &[u8]) -> u128 {
    let mut message = [0_u8; 128];
    message[..16].copy_from_slice(&namespace.to_be_bytes());
    message[16..16 + name.len()].copy_from_slice(name);
    let length = 16 + name.len();
    message[length] = 0x80;
    let blocks = (length + 9).div_ceil(64);
    message[blocks * 64 - 8..blocks * 64].copy_from_slice(&((length as u64) * 8).to_be_bytes());
    let mut hash = [
        0x6745_2301_u32,
        0xefcd_ab89,
        0x98ba_dcfe,
        0x1032_5476,
        0xc3d2_e1f0,
    ];
    for block in message[..blocks * 64].chunks_exact(64) {
        let mut words = [0_u32; 80];
        for (word, bytes) in words[..16].iter_mut().zip(block.chunks_exact(4)) {
            *word = bytes
                .iter()
                .fold(0, |value, byte| (value << 8) | u32::from(*byte));
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = hash;
        for (index, word) in words.into_iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5a82_7999),
                20..=39 => (b ^ c ^ d, 0x6ed9_eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1b_bcdc),
                _ => (b ^ c ^ d, 0xca62_c1d6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }
        for (state, part) in hash.iter_mut().zip([a, b, c, d, e]) {
            *state = state.wrapping_add(part);
        }
    }
    hash[..4]
        .iter()
        .fold(0, |value, word| (value << 32) | u128::from(*word))
}

/// A sequential finite transcript, without a clock, RNG, VM or heap oracle.
#[derive(Debug)]
pub struct ReferenceProviders {
    clocks: Vec<Result<i128, ReferenceErrorKind>>,
    entropy: Vec<Result<Vec<u8>, ReferenceErrorKind>>,
    consumed_clocks: usize,
    consumed_entropy: usize,
    closed: bool,
}

impl ReferenceProviders {
    pub fn new(
        clocks: Vec<Result<i128, ReferenceErrorKind>>,
        entropy: Vec<Result<Vec<u8>, ReferenceErrorKind>>,
    ) -> Result<Self, ReferenceError> {
        if clocks.len() > MAX_REFERENCE_PROVIDER_ROWS || entropy.len() > MAX_REFERENCE_PROVIDER_ROWS
        {
            return Err(error(ReferenceErrorKind::OutsideDomain));
        }
        if clocks.iter().any(|row| matches!(row, Err(kind) if !matches!(kind, ReferenceErrorKind::ClockUnavailable | ReferenceErrorKind::ClockFailure)))
            || entropy.iter().any(|row| match row {
                Ok(bytes) => bytes.len() > 16,
                Err(kind) => !matches!(kind, ReferenceErrorKind::EntropyUnavailable | ReferenceErrorKind::EntropyFailure),
            }) {
            return Err(error(ReferenceErrorKind::ProviderMisconfigured));
        }
        Ok(Self {
            clocks,
            entropy,
            consumed_clocks: 0,
            consumed_entropy: 0,
            closed: false,
        })
    }

    pub fn consumed(&self) -> (usize, usize) {
        (self.consumed_clocks, self.consumed_entropy)
    }

    pub fn close(&mut self) {
        self.clocks = Vec::new();
        self.entropy = Vec::new();
        self.closed = true;
    }

    pub fn v4(&mut self, provider_byte_limit: usize) -> Result<ReferenceUuid, ReferenceError> {
        self.admit(provider_byte_limit, 16)?;
        ReferenceUuid::v4(&self.entropy(16)?)
    }

    pub fn v7(&mut self, provider_byte_limit: usize) -> Result<ReferenceUuid, ReferenceError> {
        self.admit(provider_byte_limit, 10)?;
        let row = self
            .clocks
            .get(self.consumed_clocks)
            .cloned()
            .ok_or_else(|| error(ReferenceErrorKind::ClockUnavailable))?;
        self.consumed_clocks += 1;
        let milliseconds = row.map_err(error)?;
        if !(0..=MAX_MILLISECONDS).contains(&milliseconds) {
            return Err(error(ReferenceErrorKind::TimestampOutOfRange));
        }
        ReferenceUuid::v7(milliseconds, &self.entropy(10)?)
    }

    fn admit(&self, maximum: usize, required: usize) -> Result<(), ReferenceError> {
        if maximum < required {
            return Err(error(ReferenceErrorKind::ResourceLimit));
        }
        if self.closed {
            return Err(error(ReferenceErrorKind::ProviderMisconfigured));
        }
        Ok(())
    }

    fn entropy(&mut self, length: usize) -> Result<Vec<u8>, ReferenceError> {
        let row = self
            .entropy
            .get(self.consumed_entropy)
            .cloned()
            .ok_or_else(|| error(ReferenceErrorKind::EntropyUnavailable))?;
        self.consumed_entropy += 1;
        let bytes = row.map_err(error)?;
        if bytes.len() != length {
            return Err(error(ReferenceErrorKind::ProviderMisconfigured));
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCase {
    pub bytes: [u8; 16],
    pub other: [u8; 16],
    pub entropy: [u8; 16],
    pub name: Vec<u8>,
    pub milliseconds: i128,
    pub name_limit: usize,
}

fn next_word(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut word = *state;
    word = (word ^ (word >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    word ^ (word >> 31)
}

pub fn case_from_seed(seed: u64) -> GeneratedCase {
    let mut state = seed;
    let mut bytes = [0; 16];
    let mut other = [0; 16];
    let mut entropy = [0; 16];
    for output in [&mut bytes, &mut other, &mut entropy] {
        for chunk in output.chunks_exact_mut(8) {
            chunk.copy_from_slice(&next_word(&mut state).to_be_bytes());
        }
    }
    let name = (0..(seed % 97) as usize)
        .map(|_| next_word(&mut state) as u8)
        .collect::<Vec<_>>();
    let milliseconds = match seed % 6 {
        0 => -1,
        1 => 0,
        2 => MAX_MILLISECONDS,
        3 => MAX_MILLISECONDS + 1,
        _ => i128::from(next_word(&mut state) & 0xffff_ffff_ffff),
    };
    let name_limit = match seed % 3 {
        0 => 0,
        1 => MAX_REFERENCE_NAME_BYTES,
        _ => name.len().saturating_sub(1),
    };
    GeneratedCase {
        bytes,
        other,
        entropy,
        name,
        milliseconds,
        name_limit,
    }
}

pub fn seed_at_step(input: &[u8], step: usize) -> u64 {
    let input = &input[..input.len().min(MAX_UUID_FUZZ_INPUT_BYTES)];
    if input.is_empty() {
        return step as u64;
    }
    (0..16).fold(step as u64, |state, index| {
        state
            .wrapping_mul(257)
            .wrapping_add(u64::from(input[(step + index) % input.len()]))
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UuidFuzzSummary {
    pub steps: usize,
    pub generated: usize,
    pub refusals: usize,
    pub checksum: u128,
}

pub fn run_uuid_fuzz_case(input: &[u8]) -> Result<UuidFuzzSummary, String> {
    let steps = input
        .len()
        .min(MAX_UUID_FUZZ_INPUT_BYTES)
        .clamp(1, MAX_UUID_FUZZ_STEPS);
    let mut summary = UuidFuzzSummary {
        steps,
        generated: 0,
        refusals: 0,
        checksum: 0,
    };
    for step in 0..steps {
        let case = case_from_seed(seed_at_step(input, step));
        let value = ReferenceUuid::from_bytes(&case.bytes).map_err(|error| error.to_string())?;
        if ReferenceUuid::parse(&value.canonical()).map_err(|error| error.to_string())? != value {
            return Err(format!("reference round trip differs at step {step}"));
        }
        let v4 = ReferenceUuid::v4(&case.entropy).map_err(|error| error.to_string())?;
        summary.generated += 1;
        summary.checksum = summary.checksum.rotate_left(1) ^ value.0 ^ v4.0;
        for result in [
            value.v5(&case.name, case.name_limit),
            ReferenceUuid::v7(case.milliseconds, &case.entropy[..10]),
        ] {
            match result {
                Ok(value) => {
                    summary.generated += 1;
                    summary.checksum ^= value.0;
                }
                Err(ReferenceError {
                    kind:
                        ReferenceErrorKind::NameLimitExceeded | ReferenceErrorKind::TimestampOutOfRange,
                    offset: None,
                }) => summary.refusals += 1,
                Err(error) => {
                    return Err(format!(
                        "unexpected reference refusal at step {step}: {error}"
                    ));
                }
            }
        }
    }
    Ok(summary)
}
