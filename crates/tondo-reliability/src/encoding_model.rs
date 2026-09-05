//! Independent bounded reference model for `std.encoding`.
//!
//! The model deliberately does not call the production codec.  It implements
//! the wire rules directly so the scalar implementation, the hosted bridge,
//! and future optimized paths can be checked against one small oracle.

use std::fmt;

/// Maximum input consumed by one encoding fuzz replay.
pub const MAX_ENCODING_FUZZ_INPUT_BYTES: usize = 4 * 1024;
/// Maximum deterministic actions accepted by one encoding fuzz replay.
pub const MAX_ENCODING_FUZZ_STEPS: usize = 512;
/// Maximum payload sampled for each fuzz action.
const MAX_REFERENCE_PAYLOAD_BYTES: usize = 96;

/// Base64 alphabet selected by the reference policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceAlphabet {
    Standard,
    UrlSafe,
}

/// Base64 padding selected by the reference policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferencePadding {
    Required,
    Omitted,
}

/// Hexadecimal case selected by the reference policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceHexCase {
    Lower,
    Upper,
    Any,
}

/// One of the public codec policies, represented independently of the stdlib.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceCodec {
    Base64 {
        alphabet: ReferenceAlphabet,
        padding: ReferencePadding,
    },
    Hex {
        case: ReferenceHexCase,
    },
}

/// Wire errors observable by materialized and incremental decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceErrorKind {
    InvalidCharacter,
    InvalidLength,
    InvalidPadding,
    NonCanonical,
    ResourceLimit,
}

/// Error plus the number of input bytes observed before the failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceError {
    pub kind: ReferenceErrorKind,
    pub offset: usize,
}

impl fmt::Display for ReferenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} at byte {}", self.kind, self.offset)
    }
}

/// Stable observation returned by the bounded fuzz replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingFuzzSummary {
    pub steps: usize,
    pub valid_cases: usize,
    pub invalid_cases: usize,
    pub bytes_checked: usize,
    pub max_payload_bytes: usize,
}

/// Encode with the independent reference implementation and no resource cap.
pub fn encode_reference(codec: ReferenceCodec, input: &[u8]) -> Result<Vec<u8>, ReferenceError> {
    encode_reference_with_limits(codec, input, usize::MAX, usize::MAX)
}

/// Encode while applying the same aggregate input/output limits as the public
/// contract.  Limits are checked before publishing any output.
pub fn encode_reference_with_limits(
    codec: ReferenceCodec,
    input: &[u8],
    max_input_bytes: usize,
    max_output_bytes: usize,
) -> Result<Vec<u8>, ReferenceError> {
    if input.len() > max_input_bytes {
        return Err(resource_limit());
    }
    match codec {
        ReferenceCodec::Base64 { alphabet, padding } => {
            let groups = input.len() / 3;
            let remainder = input.len() % 3;
            let mut output_len = groups.checked_mul(4).ok_or_else(resource_limit)?;
            if remainder != 0 {
                output_len = output_len
                    .checked_add(match padding {
                        ReferencePadding::Required => 4,
                        ReferencePadding::Omitted => remainder + 1,
                    })
                    .ok_or_else(resource_limit)?;
            }
            if output_len > max_output_bytes {
                return Err(resource_limit());
            }
            let mut output = Vec::with_capacity(output_len);
            for quantum in input[..groups * 3].chunks_exact(3) {
                append_base64_quantum(&mut output, quantum, alphabet, ReferencePadding::Required);
            }
            if remainder != 0 {
                append_base64_quantum(&mut output, &input[groups * 3..], alphabet, padding);
            }
            Ok(output)
        }
        ReferenceCodec::Hex { case } => {
            let output_len = input.len().checked_mul(2).ok_or_else(resource_limit)?;
            if output_len > max_output_bytes {
                return Err(resource_limit());
            }
            let mut output = Vec::with_capacity(output_len);
            for byte in input {
                output.push(hex_digit(*byte >> 4, case));
                output.push(hex_digit(*byte & 0x0f, case));
            }
            Ok(output)
        }
    }
}

/// Decode with the independent reference implementation and no resource cap.
pub fn decode_reference(codec: ReferenceCodec, input: &[u8]) -> Result<Vec<u8>, ReferenceError> {
    decode_reference_with_limits(codec, input, usize::MAX, usize::MAX)
}

/// Decode while applying aggregate input/output limits.  Wire validation is
/// performed before the output limit is reported, matching the production
/// machine's error precedence for one materialized call.
pub fn decode_reference_with_limits(
    codec: ReferenceCodec,
    input: &[u8],
    max_input_bytes: usize,
    max_output_bytes: usize,
) -> Result<Vec<u8>, ReferenceError> {
    if input.len() > max_input_bytes {
        return Err(resource_limit());
    }
    let output = match codec {
        ReferenceCodec::Base64 { alphabet, padding } => decode_base64(input, alphabet, padding)?,
        ReferenceCodec::Hex { case } => decode_hex(input, case)?,
    };
    if output.len() > max_output_bytes {
        return Err(resource_limit());
    }
    Ok(output)
}

/// Return a small, deterministic set of chunk sizes.  Unit tests use all
/// sizes for short vectors; fuzzing uses this bounded set to avoid quadratic
/// work while still exercising byte, quantum, and whole-input boundaries.
pub fn bounded_chunk_sizes(input_len: usize, entropy: &[u8]) -> Vec<usize> {
    let maximum = input_len.max(1);
    let mut sizes = Vec::with_capacity(8);
    for size in [1, 2, 3, 4, maximum] {
        if size <= maximum && !sizes.contains(&size) {
            sizes.push(size);
        }
    }
    for byte in entropy.iter().take(3) {
        let size = usize::from(*byte) % maximum + 1;
        if !sizes.contains(&size) {
            sizes.push(size);
        }
    }
    sizes
}

/// Replay a bounded byte sequence against the independent codec model.
pub fn run_encoding_fuzz_case(input: &[u8]) -> Result<EncodingFuzzSummary, String> {
    let bounded = &input[..input.len().min(MAX_ENCODING_FUZZ_INPUT_BYTES)];
    let steps = bounded.len().clamp(1, MAX_ENCODING_FUZZ_STEPS);
    let input_len = bounded.len().max(1);
    let policies = [
        ReferenceCodec::Base64 {
            alphabet: ReferenceAlphabet::Standard,
            padding: ReferencePadding::Required,
        },
        ReferenceCodec::Base64 {
            alphabet: ReferenceAlphabet::UrlSafe,
            padding: ReferencePadding::Required,
        },
        ReferenceCodec::Base64 {
            alphabet: ReferenceAlphabet::UrlSafe,
            padding: ReferencePadding::Omitted,
        },
        ReferenceCodec::Hex {
            case: ReferenceHexCase::Lower,
        },
        ReferenceCodec::Hex {
            case: ReferenceHexCase::Upper,
        },
        ReferenceCodec::Hex {
            case: ReferenceHexCase::Any,
        },
    ];
    let mut valid_cases = 0;
    let mut invalid_cases = 0;
    let mut bytes_checked: usize = 0;
    let mut max_payload_bytes: usize = 0;
    for step in 0..steps {
        let selector = bounded.get(step % input_len).copied().unwrap_or_default();
        let codec = policies[(usize::from(selector) + step) % policies.len()];
        let available = bounded.len().saturating_sub(1);
        let payload_len = available.min(MAX_REFERENCE_PAYLOAD_BYTES);
        let start = if available == 0 {
            0
        } else {
            usize::from(selector) % (available + 1)
        };
        let end = (start + payload_len).min(bounded.len());
        let payload = if start < end {
            &bounded[start..end]
        } else {
            &[]
        };
        let encoded = encode_reference(codec, payload)
            .map_err(|error| format!("reference encode failed at step {step}: {error}"))?;
        let decoded = decode_reference(codec, &encoded)
            .map_err(|error| format!("reference decode failed at step {step}: {error}"))?;
        if decoded != payload {
            return Err(format!("reference roundtrip diverged at step {step}"));
        }
        valid_cases += 1;
        bytes_checked = bytes_checked.saturating_add(payload.len());
        max_payload_bytes = max_payload_bytes.max(payload.len());

        let mut malformed = encoded;
        if malformed.is_empty() {
            malformed.push(b'!');
        } else {
            malformed[0] = b'!';
        }
        if decode_reference(codec, &malformed).is_ok() {
            return Err(format!("reference accepted malformed input at step {step}"));
        }
        invalid_cases += 1;
    }
    Ok(EncodingFuzzSummary {
        steps,
        valid_cases,
        invalid_cases,
        bytes_checked,
        max_payload_bytes,
    })
}

fn resource_limit() -> ReferenceError {
    ReferenceError {
        kind: ReferenceErrorKind::ResourceLimit,
        offset: 0,
    }
}

fn decode_base64(
    input: &[u8],
    alphabet: ReferenceAlphabet,
    padding: ReferencePadding,
) -> Result<Vec<u8>, ReferenceError> {
    let mut output = Vec::new();
    let mut cursor = 0;
    while input.len() - cursor >= 4 {
        let start = cursor;
        let quantum = [
            input[cursor],
            input[cursor + 1],
            input[cursor + 2],
            input[cursor + 3],
        ];
        let (decoded, decoded_len, padded) = decode_base64_quantum(quantum, alphabet, padding)
            .map_err(|kind| ReferenceError {
                kind,
                offset: start,
            })?;
        output.extend_from_slice(&decoded[..decoded_len]);
        cursor += 4;
        if padded {
            if cursor != input.len() {
                return Err(ReferenceError {
                    kind: ReferenceErrorKind::InvalidPadding,
                    offset: cursor,
                });
            }
            return Ok(output);
        }
    }

    let remainder = &input[cursor..];
    for (index, byte) in remainder.iter().copied().enumerate() {
        if byte == b'=' {
            if padding == ReferencePadding::Required && remainder.len() == 3 && index == 2 {
                continue;
            }
            return Err(ReferenceError {
                kind: ReferenceErrorKind::InvalidPadding,
                offset: cursor + index,
            });
        }
        if decode_base64_digit(byte, alphabet).is_none() {
            return Err(ReferenceError {
                kind: ReferenceErrorKind::InvalidCharacter,
                offset: cursor + index,
            });
        }
    }

    if remainder.is_empty() {
        return Ok(output);
    }
    match padding {
        ReferencePadding::Required => Err(ReferenceError {
            kind: ReferenceErrorKind::InvalidLength,
            offset: 0,
        }),
        ReferencePadding::Omitted => {
            if remainder.len() == 1 {
                return Err(ReferenceError {
                    kind: ReferenceErrorKind::InvalidLength,
                    offset: 0,
                });
            }
            let mut quantum = [b'='; 4];
            quantum[..remainder.len()].copy_from_slice(remainder);
            let (decoded, decoded_len, _) =
                decode_base64_quantum(quantum, alphabet, ReferencePadding::Required)
                    .map_err(|kind| ReferenceError { kind, offset: 0 })?;
            output.extend_from_slice(&decoded[..decoded_len]);
            Ok(output)
        }
    }
}

fn decode_hex(input: &[u8], case: ReferenceHexCase) -> Result<Vec<u8>, ReferenceError> {
    let mut output = Vec::with_capacity(input.len() / 2);
    let mut high = None;
    for (offset, byte) in input.iter().copied().enumerate() {
        let nibble =
            decode_hex_digit(byte, case).map_err(|kind| ReferenceError { kind, offset })?;
        if let Some(high_nibble) = high.take() {
            output.push((high_nibble << 4) | nibble);
        } else {
            high = Some(nibble);
        }
    }
    if high.is_some() {
        return Err(ReferenceError {
            kind: ReferenceErrorKind::InvalidLength,
            offset: 0,
        });
    }
    Ok(output)
}

fn append_base64_quantum(
    output: &mut Vec<u8>,
    input: &[u8],
    alphabet: ReferenceAlphabet,
    padding: ReferencePadding,
) {
    let first = input[0];
    let second = input.get(1).copied().unwrap_or_default();
    let third = input.get(2).copied().unwrap_or_default();
    output.push(base64_digit((first >> 2) & 0x3f, alphabet));
    output.push(base64_digit(
        ((first & 0x03) << 4) | (second >> 4),
        alphabet,
    ));
    if input.len() > 1 {
        output.push(base64_digit(
            ((second & 0x0f) << 2) | (third >> 6),
            alphabet,
        ));
    } else if padding == ReferencePadding::Required {
        output.push(b'=');
    }
    if input.len() > 2 {
        output.push(base64_digit(third & 0x3f, alphabet));
    } else if padding == ReferencePadding::Required {
        output.push(b'=');
    }
}

fn decode_base64_quantum(
    quantum: [u8; 4],
    alphabet: ReferenceAlphabet,
    padding: ReferencePadding,
) -> Result<([u8; 3], usize, bool), ReferenceErrorKind> {
    if quantum[0] == b'=' || quantum[1] == b'=' {
        return Err(ReferenceErrorKind::InvalidPadding);
    }
    let first =
        decode_base64_digit(quantum[0], alphabet).ok_or(ReferenceErrorKind::InvalidCharacter)?;
    let second =
        decode_base64_digit(quantum[1], alphabet).ok_or(ReferenceErrorKind::InvalidCharacter)?;
    let second_padding = quantum[2] == b'=';
    let third_padding = quantum[3] == b'=';
    let padding_count = match (second_padding, third_padding) {
        (true, true) => 2,
        (false, true) => 1,
        (true, false) => return Err(ReferenceErrorKind::InvalidPadding),
        (false, false) => 0,
    };
    if padding == ReferencePadding::Omitted && padding_count != 0 {
        return Err(ReferenceErrorKind::InvalidPadding);
    }
    let third = if second_padding {
        0
    } else {
        decode_base64_digit(quantum[2], alphabet).ok_or(ReferenceErrorKind::InvalidCharacter)?
    };
    let fourth = if third_padding {
        0
    } else {
        decode_base64_digit(quantum[3], alphabet).ok_or(ReferenceErrorKind::InvalidCharacter)?
    };
    if (padding_count == 2 && (second & 0x0f) != 0) || (padding_count == 1 && (third & 0x03) != 0) {
        return Err(ReferenceErrorKind::NonCanonical);
    }
    Ok((
        [
            (first << 2) | (second >> 4),
            (second << 4) | (third >> 2),
            (third << 6) | fourth,
        ],
        3 - padding_count,
        padding_count != 0,
    ))
}

fn base64_digit(value: u8, alphabet: ReferenceAlphabet) -> u8 {
    const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    match alphabet {
        ReferenceAlphabet::Standard => STANDARD[value as usize],
        ReferenceAlphabet::UrlSafe => URL_SAFE[value as usize],
    }
}

fn decode_base64_digit(byte: u8, alphabet: ReferenceAlphabet) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' if alphabet == ReferenceAlphabet::Standard => Some(62),
        b'/' if alphabet == ReferenceAlphabet::Standard => Some(63),
        b'-' if alphabet == ReferenceAlphabet::UrlSafe => Some(62),
        b'_' if alphabet == ReferenceAlphabet::UrlSafe => Some(63),
        _ => None,
    }
}

fn hex_digit(value: u8, case: ReferenceHexCase) -> u8 {
    match (value, case) {
        (0..=9, _) => b'0' + value,
        (10..=15, ReferenceHexCase::Upper) => b'A' + (value - 10),
        (10..=15, ReferenceHexCase::Lower | ReferenceHexCase::Any) => b'a' + (value - 10),
        _ => unreachable!("hex nibble is always in range"),
    }
}

fn decode_hex_digit(byte: u8, case: ReferenceHexCase) -> Result<u8, ReferenceErrorKind> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' if matches!(case, ReferenceHexCase::Lower | ReferenceHexCase::Any) => {
            Ok(byte - b'a' + 10)
        }
        b'A'..=b'F' if matches!(case, ReferenceHexCase::Upper | ReferenceHexCase::Any) => {
            Ok(byte - b'A' + 10)
        }
        b'a'..=b'f' | b'A'..=b'F' => Err(ReferenceErrorKind::NonCanonical),
        _ => Err(ReferenceErrorKind::InvalidCharacter),
    }
}
