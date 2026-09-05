//! Bounded Base64 and hexadecimal codecs for the standard-library owner.
//!
//! The scalar machines in this module are the single semantic oracle for
//! materialised and incremental operations. They own no input buffer: every
//! chunk is copied only into the returned Bytes, while the fixed-size carry
//! remains inside the affine stream handle.

use std::fmt;

use crate::io::{self, ReadResult, Reader, Writer};
use crate::serialization::Bytes;

const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
const STREAM_READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64Alphabet {
    Standard,
    UrlSafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64Padding {
    Required,
    Omitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexCase {
    Lower,
    Upper,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodingErrorKind {
    InvalidLimit,
    InvalidCharacter,
    InvalidLength,
    InvalidPadding,
    NonCanonical,
    ResourceLimit,
    Io(io::IoError),
    Closed,
    NoProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodingError {
    pub kind: EncodingErrorKind,
    pub offset: usize,
}

impl EncodingError {
    fn new(kind: EncodingErrorKind, offset: usize) -> Self {
        Self { kind, offset }
    }
}

impl fmt::Display for EncodingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?} at byte {}", self.kind, self.offset)
    }
}

impl std::error::Error for EncodingError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodingLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
}

impl Default for EncodingLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: DEFAULT_MAX_BYTES,
            max_output_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl EncodingLimits {
    /// Create finite limits. Zero is valid and only permits an empty
    /// transformation, matching the source-level contract.
    pub fn create(max_input_bytes: usize, max_output_bytes: usize) -> Result<Self, EncodingError> {
        Ok(Self {
            max_input_bytes,
            max_output_bytes,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base64Options {
    pub alphabet: Base64Alphabet,
    pub padding: Base64Padding,
    pub limits: EncodingLimits,
}

impl Base64Options {
    pub const fn create(
        alphabet: Base64Alphabet,
        padding: Base64Padding,
        limits: EncodingLimits,
    ) -> Self {
        Self {
            alphabet,
            padding,
            limits,
        }
    }

    pub const fn standard(limits: EncodingLimits) -> Self {
        Self::create(Base64Alphabet::Standard, Base64Padding::Required, limits)
    }

    pub const fn url_safe(limits: EncodingLimits) -> Self {
        Self::create(Base64Alphabet::UrlSafe, Base64Padding::Required, limits)
    }

    pub const fn url_safe_unpadded(limits: EncodingLimits) -> Self {
        Self::create(Base64Alphabet::UrlSafe, Base64Padding::Omitted, limits)
    }

    pub fn encode(self, input: &Bytes) -> Result<Bytes, EncodingError> {
        let mut encoder = self.encoder()?;
        let first = encoder.push(input)?;
        let last = encoder.finish()?;
        Ok(join_bytes(first, last))
    }

    pub fn decode(self, input: &Bytes) -> Result<Bytes, EncodingError> {
        let mut decoder = self.decoder()?;
        let first = decoder.push(input)?;
        let last = decoder.finish()?;
        Ok(join_bytes(first, last))
    }

    pub fn encode_to<W: Writer>(self, input: &Bytes, writer: &mut W) -> Result<(), EncodingError> {
        let mut encoder = self.encoder()?;
        let first = encoder.push(input)?;
        write_encoded(writer, first.as_slice(), encoder.input_bytes)?;
        let last = encoder.finish()?;
        write_encoded(writer, last.as_slice(), encoder.input_bytes)?;
        writer
            .flush()
            .map_err(|error| EncodingError::new(EncodingErrorKind::Io(error), encoder.input_bytes))
    }

    pub fn decode_from<R: Reader>(self, reader: &mut R) -> Result<Bytes, EncodingError> {
        let mut decoder = self.decoder()?;
        let mut output = Vec::new();
        let request = STREAM_READ_CHUNK.min(self.limits.max_input_bytes.max(1));
        loop {
            let chunk = match reader.read(request) {
                Ok(ReadResult::Eof) => break,
                Ok(ReadResult::Data(chunk)) => {
                    if chunk.is_empty() || chunk.len() > request {
                        decoder.terminal = true;
                        return Err(EncodingError::new(
                            EncodingErrorKind::Io(io::IoError::InvalidData),
                            decoder.input_bytes,
                        ));
                    }
                    chunk
                }
                Err(error) => {
                    decoder.terminal = true;
                    return Err(EncodingError::new(
                        EncodingErrorKind::Io(error),
                        decoder.input_bytes,
                    ));
                }
            };
            let produced = decoder.push(&Bytes::new(chunk))?;
            output.extend_from_slice(produced.as_slice());
        }
        let produced = decoder.finish()?;
        output.extend_from_slice(produced.as_slice());
        Ok(Bytes::new(output))
    }

    pub fn encoder(self) -> Result<Base64Encoder, EncodingError> {
        Ok(Base64Encoder::new(self))
    }

    pub fn decoder(self) -> Result<Base64Decoder, EncodingError> {
        Ok(Base64Decoder::new(self))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HexOptions {
    pub case: HexCase,
    pub limits: EncodingLimits,
}

impl HexOptions {
    pub const fn create(case: HexCase, limits: EncodingLimits) -> Self {
        Self { case, limits }
    }

    pub const fn lower(limits: EncodingLimits) -> Self {
        Self::create(HexCase::Lower, limits)
    }

    pub const fn upper(limits: EncodingLimits) -> Self {
        Self::create(HexCase::Upper, limits)
    }

    pub const fn any_case(limits: EncodingLimits) -> Self {
        Self::create(HexCase::Any, limits)
    }

    pub fn encode(self, input: &Bytes) -> Result<Bytes, EncodingError> {
        let mut encoder = self.encoder()?;
        let first = encoder.push(input)?;
        let last = encoder.finish()?;
        Ok(join_bytes(first, last))
    }

    pub fn decode(self, input: &Bytes) -> Result<Bytes, EncodingError> {
        let mut decoder = self.decoder()?;
        let first = decoder.push(input)?;
        let last = decoder.finish()?;
        Ok(join_bytes(first, last))
    }

    pub fn encode_to<W: Writer>(self, input: &Bytes, writer: &mut W) -> Result<(), EncodingError> {
        let mut encoder = self.encoder()?;
        let first = encoder.push(input)?;
        write_encoded(writer, first.as_slice(), encoder.input_bytes)?;
        let last = encoder.finish()?;
        write_encoded(writer, last.as_slice(), encoder.input_bytes)?;
        writer
            .flush()
            .map_err(|error| EncodingError::new(EncodingErrorKind::Io(error), encoder.input_bytes))
    }

    pub fn decode_from<R: Reader>(self, reader: &mut R) -> Result<Bytes, EncodingError> {
        let mut decoder = self.decoder()?;
        let mut output = Vec::new();
        let request = STREAM_READ_CHUNK.min(self.limits.max_input_bytes.max(1));
        loop {
            let chunk = match reader.read(request) {
                Ok(ReadResult::Eof) => break,
                Ok(ReadResult::Data(chunk)) => {
                    if chunk.is_empty() || chunk.len() > request {
                        decoder.terminal = true;
                        return Err(EncodingError::new(
                            EncodingErrorKind::Io(io::IoError::InvalidData),
                            decoder.input_bytes,
                        ));
                    }
                    chunk
                }
                Err(error) => {
                    decoder.terminal = true;
                    return Err(EncodingError::new(
                        EncodingErrorKind::Io(error),
                        decoder.input_bytes,
                    ));
                }
            };
            let produced = decoder.push(&Bytes::new(chunk))?;
            output.extend_from_slice(produced.as_slice());
        }
        let produced = decoder.finish()?;
        output.extend_from_slice(produced.as_slice());
        Ok(Bytes::new(output))
    }

    pub fn encoder(self) -> Result<HexEncoder, EncodingError> {
        Ok(HexEncoder::new(self))
    }

    pub fn decoder(self) -> Result<HexDecoder, EncodingError> {
        Ok(HexDecoder::new(self))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base64Encoder {
    options: Base64Options,
    carry: [u8; 2],
    carry_len: usize,
    input_bytes: usize,
    output_bytes: usize,
    terminal: bool,
}

impl Base64Encoder {
    fn new(options: Base64Options) -> Self {
        Self {
            options,
            carry: [0; 2],
            carry_len: 0,
            input_bytes: 0,
            output_bytes: 0,
            terminal: false,
        }
    }

    pub fn push(&mut self, chunk: &Bytes) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        if chunk.as_slice().is_empty() {
            return Ok(Bytes::default());
        }
        let next_input = match checked_limit(
            self.input_bytes,
            chunk.as_slice().len(),
            self.options.limits.max_input_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let mut combined = Vec::with_capacity(self.carry_len + chunk.as_slice().len());
        combined.extend_from_slice(&self.carry[..self.carry_len]);
        combined.extend_from_slice(chunk.as_slice());
        let full_len = combined.len() / 3 * 3;
        let output_len = match checked_mul(full_len / 3, 4) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let next_output = match checked_limit(
            self.output_bytes,
            output_len,
            self.options.limits.max_output_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let mut output = Vec::with_capacity(output_len);
        for quantum in combined[..full_len].chunks_exact(3) {
            append_base64_quantum(
                &mut output,
                quantum,
                self.options.alphabet,
                Base64Padding::Required,
            );
        }
        let remainder = &combined[full_len..];
        let mut carry = [0; 2];
        carry[..remainder.len()].copy_from_slice(remainder);
        self.carry = carry;
        self.carry_len = remainder.len();
        self.input_bytes = next_input;
        self.output_bytes = next_output;
        Ok(Bytes::new(output))
    }

    pub fn finish(&mut self) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        let output_len = match self.carry_len {
            0 => 0,
            1 | 2 => match self.options.padding {
                Base64Padding::Required => 4,
                Base64Padding::Omitted => self.carry_len + 1,
            },
            _ => unreachable!("Base64 carry is at most two bytes"),
        };
        let next_output = match checked_limit(
            self.output_bytes,
            output_len,
            self.options.limits.max_output_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let mut output = Vec::with_capacity(output_len);
        if self.carry_len > 0 {
            append_base64_quantum(
                &mut output,
                &self.carry[..self.carry_len],
                self.options.alphabet,
                self.options.padding,
            );
            if self.options.padding == Base64Padding::Omitted {
                output.truncate(output_len);
            }
        }
        self.output_bytes = next_output;
        self.carry_len = 0;
        self.terminal = true;
        Ok(Bytes::new(output))
    }

    fn ensure_open(&self) -> Result<(), EncodingError> {
        if self.terminal {
            Err(EncodingError::new(EncodingErrorKind::Closed, 0))
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self, kind: EncodingErrorKind, offset: usize) -> Result<T, EncodingError> {
        self.terminal = true;
        Err(EncodingError::new(kind, offset))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base64Decoder {
    options: Base64Options,
    pending: [u8; 3],
    pending_len: usize,
    input_bytes: usize,
    output_bytes: usize,
    finished_padding: bool,
    terminal: bool,
}

impl Base64Decoder {
    fn new(options: Base64Options) -> Self {
        Self {
            options,
            pending: [0; 3],
            pending_len: 0,
            input_bytes: 0,
            output_bytes: 0,
            finished_padding: false,
            terminal: false,
        }
    }

    pub fn push(&mut self, chunk: &Bytes) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        if chunk.as_slice().is_empty() {
            return Ok(Bytes::default());
        }
        let next_input = match checked_limit(
            self.input_bytes,
            chunk.as_slice().len(),
            self.options.limits.max_input_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let previous_pending_len = self.pending_len;
        let mut combined = Vec::with_capacity(previous_pending_len + chunk.as_slice().len());
        combined.extend_from_slice(&self.pending[..previous_pending_len]);
        combined.extend_from_slice(chunk.as_slice());
        let mut output = Vec::new();
        let mut cursor = 0;
        let mut finished_padding = self.finished_padding;
        while combined.len() - cursor >= 4 {
            if finished_padding {
                return self.fail(
                    EncodingErrorKind::InvalidPadding,
                    self.input_bytes + cursor.saturating_sub(previous_pending_len),
                );
            }
            let quantum = [
                combined[cursor],
                combined[cursor + 1],
                combined[cursor + 2],
                combined[cursor + 3],
            ];
            let (decoded, decoded_len, padded) =
                match decode_base64_quantum(quantum, self.options.alphabet, self.options.padding) {
                    Ok(value) => value,
                    Err(kind) => {
                        return self.fail(
                            kind,
                            self.input_bytes + cursor.saturating_sub(previous_pending_len),
                        );
                    }
                };
            output.extend_from_slice(&decoded[..decoded_len]);
            cursor += 4;
            if padded {
                finished_padding = true;
                if cursor != combined.len() {
                    return self.fail(
                        EncodingErrorKind::InvalidPadding,
                        self.input_bytes + cursor.saturating_sub(previous_pending_len),
                    );
                }
            }
        }
        for (index, byte) in combined[cursor..].iter().enumerate() {
            if finished_padding {
                return self.fail(
                    EncodingErrorKind::InvalidPadding,
                    self.input_bytes + (cursor + index).saturating_sub(previous_pending_len),
                );
            }
            if *byte == b'=' {
                if self.options.padding == Base64Padding::Required
                    && remainder_is_incomplete_padded(&combined[cursor..], index)
                {
                    continue;
                }
                return self.fail(
                    EncodingErrorKind::InvalidPadding,
                    self.input_bytes + (cursor + index).saturating_sub(previous_pending_len),
                );
            }
            if decode_base64_digit(*byte, self.options.alphabet).is_none() {
                return self.fail(
                    EncodingErrorKind::InvalidCharacter,
                    self.input_bytes + (cursor + index).saturating_sub(previous_pending_len),
                );
            }
        }
        let remainder = &combined[cursor..];
        let next_output = match self.output_bytes.checked_add(output.len()) {
            Some(value) if value <= self.options.limits.max_output_bytes => value,
            _ => return self.fail(EncodingErrorKind::ResourceLimit, 0),
        };
        let mut pending = [0; 3];
        pending[..remainder.len()].copy_from_slice(remainder);
        self.pending = pending;
        self.pending_len = remainder.len();
        self.input_bytes = next_input;
        self.output_bytes = next_output;
        self.finished_padding = finished_padding;
        Ok(Bytes::new(output))
    }

    pub fn finish(&mut self) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        let mut output = Vec::new();
        if self.pending_len > 0 {
            match self.options.padding {
                Base64Padding::Required => {
                    return self.fail(EncodingErrorKind::InvalidLength, 0);
                }
                Base64Padding::Omitted => {
                    if self.pending_len == 1 {
                        return self.fail(EncodingErrorKind::InvalidLength, 0);
                    }
                    let mut quantum = [b'='; 4];
                    quantum[..self.pending_len].copy_from_slice(&self.pending[..self.pending_len]);
                    let (decoded, decoded_len, _) = match decode_base64_quantum(
                        quantum,
                        self.options.alphabet,
                        Base64Padding::Required,
                    ) {
                        Ok(value) => value,
                        Err(kind) => return self.fail(kind, 0),
                    };
                    output.extend_from_slice(&decoded[..decoded_len]);
                }
            }
        }
        let next_output = match self.output_bytes.checked_add(output.len()) {
            Some(value) if value <= self.options.limits.max_output_bytes => value,
            _ => return self.fail(EncodingErrorKind::ResourceLimit, 0),
        };
        self.output_bytes = next_output;
        self.pending_len = 0;
        self.terminal = true;
        Ok(Bytes::new(output))
    }

    fn ensure_open(&self) -> Result<(), EncodingError> {
        if self.terminal {
            Err(EncodingError::new(EncodingErrorKind::Closed, 0))
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self, kind: EncodingErrorKind, offset: usize) -> Result<T, EncodingError> {
        self.terminal = true;
        Err(EncodingError::new(kind, offset))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexEncoder {
    options: HexOptions,
    input_bytes: usize,
    output_bytes: usize,
    terminal: bool,
}

impl HexEncoder {
    fn new(options: HexOptions) -> Self {
        Self {
            options,
            input_bytes: 0,
            output_bytes: 0,
            terminal: false,
        }
    }

    pub fn push(&mut self, chunk: &Bytes) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        if chunk.as_slice().is_empty() {
            return Ok(Bytes::default());
        }
        let next_input = match checked_limit(
            self.input_bytes,
            chunk.as_slice().len(),
            self.options.limits.max_input_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let output_len = match checked_mul(chunk.as_slice().len(), 2) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let next_output = match checked_limit(
            self.output_bytes,
            output_len,
            self.options.limits.max_output_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let mut output = Vec::with_capacity(output_len);
        for byte in chunk.as_slice() {
            output.push(hex_digit(*byte >> 4, self.options.case));
            output.push(hex_digit(*byte & 0x0f, self.options.case));
        }
        self.input_bytes = next_input;
        self.output_bytes = next_output;
        Ok(Bytes::new(output))
    }

    pub fn finish(&mut self) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        self.terminal = true;
        Ok(Bytes::default())
    }

    fn ensure_open(&self) -> Result<(), EncodingError> {
        if self.terminal {
            Err(EncodingError::new(EncodingErrorKind::Closed, 0))
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self, kind: EncodingErrorKind, offset: usize) -> Result<T, EncodingError> {
        self.terminal = true;
        Err(EncodingError::new(kind, offset))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HexDecoder {
    options: HexOptions,
    pending: Option<u8>,
    input_bytes: usize,
    output_bytes: usize,
    terminal: bool,
}

impl HexDecoder {
    fn new(options: HexOptions) -> Self {
        Self {
            options,
            pending: None,
            input_bytes: 0,
            output_bytes: 0,
            terminal: false,
        }
    }

    pub fn push(&mut self, chunk: &Bytes) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        if chunk.as_slice().is_empty() {
            return Ok(Bytes::default());
        }
        let next_input = match checked_limit(
            self.input_bytes,
            chunk.as_slice().len(),
            self.options.limits.max_input_bytes,
        ) {
            Ok(value) => value,
            Err(error) => return self.fail(error.kind, error.offset),
        };
        let mut pending = self.pending;
        let output_capacity = chunk.as_slice().len() / 2 + chunk.as_slice().len() % 2;
        let mut output = Vec::with_capacity(output_capacity);
        for (index, byte) in chunk.as_slice().iter().copied().enumerate() {
            let nibble = match decode_hex_digit(byte, self.options.case) {
                Ok(value) => value,
                Err(kind) => {
                    return self.fail(kind, self.input_bytes + index);
                }
            };
            if let Some(high) = pending.take() {
                output.push((high << 4) | nibble);
            } else {
                pending = Some(nibble);
            }
        }
        let next_output = match self.output_bytes.checked_add(output.len()) {
            Some(value) if value <= self.options.limits.max_output_bytes => value,
            _ => return self.fail(EncodingErrorKind::ResourceLimit, 0),
        };
        self.pending = pending;
        self.input_bytes = next_input;
        self.output_bytes = next_output;
        Ok(Bytes::new(output))
    }

    pub fn finish(&mut self) -> Result<Bytes, EncodingError> {
        self.ensure_open()?;
        if self.pending.is_some() {
            return self.fail(EncodingErrorKind::InvalidLength, 0);
        }
        self.terminal = true;
        Ok(Bytes::default())
    }

    fn ensure_open(&self) -> Result<(), EncodingError> {
        if self.terminal {
            Err(EncodingError::new(EncodingErrorKind::Closed, 0))
        } else {
            Ok(())
        }
    }

    fn fail<T>(&mut self, kind: EncodingErrorKind, offset: usize) -> Result<T, EncodingError> {
        self.terminal = true;
        Err(EncodingError::new(kind, offset))
    }
}

fn join_bytes(first: Bytes, second: Bytes) -> Bytes {
    let mut output = first.into_vec();
    output.extend_from_slice(second.as_slice());
    Bytes::new(output)
}

fn checked_mul(value: usize, multiplier: usize) -> Result<usize, EncodingError> {
    value
        .checked_mul(multiplier)
        .ok_or_else(|| EncodingError::new(EncodingErrorKind::ResourceLimit, 0))
}

fn checked_limit(current: usize, added: usize, limit: usize) -> Result<usize, EncodingError> {
    let next = current
        .checked_add(added)
        .ok_or_else(|| EncodingError::new(EncodingErrorKind::ResourceLimit, 0))?;
    if next > limit {
        Err(EncodingError::new(EncodingErrorKind::ResourceLimit, 0))
    } else {
        Ok(next)
    }
}

fn write_encoded<W: Writer>(
    writer: &mut W,
    bytes: &[u8],
    offset: usize,
) -> Result<(), EncodingError> {
    let mut cursor = 0;
    while cursor < bytes.len() {
        let written = writer
            .write(&bytes[cursor..])
            .map_err(|error| EncodingError::new(EncodingErrorKind::Io(error), offset))?;
        if written == 0 {
            return Err(EncodingError::new(EncodingErrorKind::NoProgress, offset));
        }
        if written > bytes.len() - cursor {
            return Err(EncodingError::new(
                EncodingErrorKind::Io(io::IoError::InvalidData),
                offset,
            ));
        }
        cursor += written;
    }
    Ok(())
}

fn remainder_is_incomplete_padded(remainder: &[u8], index: usize) -> bool {
    remainder.len() == 3 && index == 2
}

fn append_base64_quantum(
    output: &mut Vec<u8>,
    input: &[u8],
    alphabet: Base64Alphabet,
    padding: Base64Padding,
) {
    const STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    const URL_SAFE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let table = match alphabet {
        Base64Alphabet::Standard => STANDARD,
        Base64Alphabet::UrlSafe => URL_SAFE,
    };
    let first = input[0];
    let second = input.get(1).copied().unwrap_or(0);
    let third = input.get(2).copied().unwrap_or(0);
    output.push(table[(first >> 2) as usize]);
    output.push(table[((first & 0x03) << 4 | (second >> 4)) as usize]);
    if input.len() > 1 {
        output.push(table[((second & 0x0f) << 2 | (third >> 6)) as usize]);
    } else if padding == Base64Padding::Required {
        output.push(b'=');
    }
    if input.len() > 2 {
        output.push(table[(third & 0x3f) as usize]);
    } else if padding == Base64Padding::Required {
        output.push(b'=');
    }
}

fn decode_base64_quantum(
    quantum: [u8; 4],
    alphabet: Base64Alphabet,
    padding: Base64Padding,
) -> Result<([u8; 3], usize, bool), EncodingErrorKind> {
    if quantum[0] == b'=' || quantum[1] == b'=' {
        return Err(EncodingErrorKind::InvalidPadding);
    }
    let first =
        decode_base64_digit(quantum[0], alphabet).ok_or(EncodingErrorKind::InvalidCharacter)?;
    let second =
        decode_base64_digit(quantum[1], alphabet).ok_or(EncodingErrorKind::InvalidCharacter)?;
    let second_padding = quantum[2] == b'=';
    let third_padding = quantum[3] == b'=';
    let padding_count = match (second_padding, third_padding) {
        (true, true) => 2,
        (false, true) => 1,
        (true, false) => return Err(EncodingErrorKind::InvalidPadding),
        (false, false) => 0,
    };
    if padding == Base64Padding::Omitted && padding_count != 0 {
        return Err(EncodingErrorKind::InvalidPadding);
    }
    let third = if second_padding {
        0
    } else {
        decode_base64_digit(quantum[2], alphabet).ok_or(EncodingErrorKind::InvalidCharacter)?
    };
    let fourth = if third_padding {
        0
    } else {
        decode_base64_digit(quantum[3], alphabet).ok_or(EncodingErrorKind::InvalidCharacter)?
    };
    if (padding_count == 2 && (second & 0x0f) != 0) || (padding_count == 1 && (third & 0x03) != 0) {
        return Err(EncodingErrorKind::NonCanonical);
    }
    let decoded = [
        (first << 2) | (second >> 4),
        (second << 4) | (third >> 2),
        (third << 6) | fourth,
    ];
    Ok((decoded, 3 - padding_count, padding_count != 0))
}

fn decode_base64_digit(byte: u8, alphabet: Base64Alphabet) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' if alphabet == Base64Alphabet::Standard => Some(62),
        b'/' if alphabet == Base64Alphabet::Standard => Some(63),
        b'-' if alphabet == Base64Alphabet::UrlSafe => Some(62),
        b'_' if alphabet == Base64Alphabet::UrlSafe => Some(63),
        _ => None,
    }
}

fn hex_digit(value: u8, case: HexCase) -> u8 {
    match (value, case) {
        (0..=9, _) => b'0' + value,
        (10..=15, HexCase::Upper) => b'A' + (value - 10),
        (10..=15, HexCase::Lower | HexCase::Any) => b'a' + (value - 10),
        _ => unreachable!("hex nibble is always in range"),
    }
}

fn decode_hex_digit(byte: u8, case: HexCase) -> Result<u8, EncodingErrorKind> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' if matches!(case, HexCase::Lower | HexCase::Any) => Ok(byte - b'a' + 10),
        b'A'..=b'F' if matches!(case, HexCase::Upper | HexCase::Any) => Ok(byte - b'A' + 10),
        b'a'..=b'f' | b'A'..=b'F' => Err(EncodingErrorKind::NonCanonical),
        _ => Err(EncodingErrorKind::InvalidCharacter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{SliceReader, VecWriter};

    fn bytes(value: &[u8]) -> Bytes {
        Bytes::from_slice(value)
    }

    #[test]
    fn base64_policies_match_rfc4648_and_url_safe_vectors() {
        let limits = EncodingLimits::default();
        let standard = Base64Options::standard(limits);
        let url_safe = Base64Options::url_safe_unpadded(limits);
        for (raw, encoded) in [
            (b"".as_slice(), ""),
            (b"f".as_slice(), "Zg=="),
            (b"fo".as_slice(), "Zm8="),
            (b"foo".as_slice(), "Zm9v"),
            (b"foobar".as_slice(), "Zm9vYmFy"),
        ] {
            assert_eq!(
                standard.encode(&bytes(raw)).unwrap().as_slice(),
                encoded.as_bytes()
            );
            assert_eq!(
                standard
                    .decode(&bytes(encoded.as_bytes()))
                    .unwrap()
                    .as_slice(),
                raw
            );
        }
        assert_eq!(
            url_safe.encode(&bytes(b"\xfb\xff")).unwrap().as_slice(),
            b"-_8"
        );
        assert_eq!(
            url_safe.decode(&bytes(b"-_8")).unwrap().as_slice(),
            b"\xfb\xff"
        );
    }

    #[test]
    fn base64_decoder_rejects_alphabet_padding_and_noncanonical_bits() {
        let limits = EncodingLimits::default();
        let standard = Base64Options::standard(limits);
        for (input, kind) in [
            (b"Zg".as_slice(), EncodingErrorKind::InvalidLength),
            (b"Zg=".as_slice(), EncodingErrorKind::InvalidLength),
            (b"Zh==".as_slice(), EncodingErrorKind::NonCanonical),
            (b"Zg==x".as_slice(), EncodingErrorKind::InvalidPadding),
            (b"Zg==\n".as_slice(), EncodingErrorKind::InvalidPadding),
            (b"-_8=".as_slice(), EncodingErrorKind::InvalidCharacter),
            (b"Z===".as_slice(), EncodingErrorKind::InvalidPadding),
        ] {
            assert_eq!(standard.decode(&bytes(input)).unwrap_err().kind, kind);
        }
        let unpadded = Base64Options::url_safe_unpadded(limits);
        assert_eq!(
            unpadded.decode(&bytes(b"Zg==")).unwrap_err().kind,
            EncodingErrorKind::InvalidPadding
        );
        assert_eq!(
            unpadded.decode(&bytes(b"Z")).unwrap_err().kind,
            EncodingErrorKind::InvalidLength
        );
    }

    #[test]
    fn base64_streaming_is_chunk_invariant_and_terminal() {
        let options = Base64Options::url_safe_unpadded(EncodingLimits::default());
        let source = bytes(b"tondo streaming");
        let expected = options.encode(&source).unwrap();
        for split in 1..=source.as_slice().len() {
            let mut encoder = options.encoder().unwrap();
            let mut output = Vec::new();
            for chunk in source.as_slice().chunks(split) {
                output.extend_from_slice(encoder.push(&bytes(chunk)).unwrap().as_slice());
            }
            output.extend_from_slice(encoder.finish().unwrap().as_slice());
            assert_eq!(output, expected.as_slice());
            assert_eq!(
                encoder.push(&bytes(b"x")).unwrap_err().kind,
                EncodingErrorKind::Closed
            );
        }
        let mut decoder = options.decoder().unwrap();
        let mut decoded = Vec::new();
        for byte in expected.as_slice() {
            decoded.extend_from_slice(decoder.push(&bytes(&[*byte])).unwrap().as_slice());
        }
        decoded.extend_from_slice(decoder.finish().unwrap().as_slice());
        assert_eq!(decoded, source.as_slice());
    }

    #[test]
    fn base64_limits_are_atomic_and_empty_zero_limit_is_valid() {
        let limits = EncodingLimits::create(2, 2).unwrap();
        let options = Base64Options::standard(limits);
        assert_eq!(options.encode(&bytes(b"")).unwrap().as_slice(), b"");
        let mut encoder = options.encoder().unwrap();
        assert_eq!(
            encoder.push(&bytes(b"abc")).unwrap_err(),
            EncodingError::new(EncodingErrorKind::ResourceLimit, 0)
        );
        assert_eq!(
            encoder.push(&bytes(b"")).unwrap_err().kind,
            EncodingErrorKind::Closed
        );
        let mut decoder = options.decoder().unwrap();
        decoder.push(&bytes(b"Zg")).unwrap();
        assert_eq!(
            decoder.push(&bytes(b"==")).unwrap_err().kind,
            EncodingErrorKind::ResourceLimit
        );
    }

    #[test]
    fn hexadecimal_policies_are_strict_and_any_case_is_lowercase() {
        let limits = EncodingLimits::default();
        assert_eq!(
            HexOptions::lower(limits)
                .encode(&bytes(&[0, 0xab, 0xff]))
                .unwrap()
                .as_slice(),
            b"00abff"
        );
        assert_eq!(
            HexOptions::upper(limits)
                .encode(&bytes(&[0, 0xab, 0xff]))
                .unwrap()
                .as_slice(),
            b"00ABFF"
        );
        let any = HexOptions::any_case(limits);
        assert_eq!(
            any.decode(&bytes(b"00aBff")).unwrap().as_slice(),
            &[0, 0xab, 0xff]
        );
        assert_eq!(any.encode(&bytes(&[0xab])).unwrap().as_slice(), b"ab");
        assert_eq!(
            HexOptions::lower(limits)
                .decode(&bytes(b"00AB"))
                .unwrap_err()
                .kind,
            EncodingErrorKind::NonCanonical
        );
        assert_eq!(
            any.decode(&bytes(b"0x")).unwrap_err().kind,
            EncodingErrorKind::InvalidCharacter
        );
        assert_eq!(
            any.decode(&bytes(b"f")).unwrap_err().kind,
            EncodingErrorKind::InvalidLength
        );
    }

    #[test]
    fn hexadecimal_streaming_and_chunk_limits_are_stable() {
        let options = HexOptions::upper(EncodingLimits::default());
        let source = bytes(&[0, 1, 0xfe, 0xff]);
        let expected = options.encode(&source).unwrap();
        let mut encoder = options.encoder().unwrap();
        let mut encoded = Vec::new();
        for chunk in source.as_slice().chunks(1) {
            encoded.extend_from_slice(encoder.push(&bytes(chunk)).unwrap().as_slice());
        }
        encoded.extend_from_slice(encoder.finish().unwrap().as_slice());
        assert_eq!(encoded, expected.as_slice());

        let mut decoder = options.decoder().unwrap();
        let mut decoded = Vec::new();
        for chunk in encoded.chunks(3) {
            decoded.extend_from_slice(decoder.push(&bytes(chunk)).unwrap().as_slice());
        }
        decoded.extend_from_slice(decoder.finish().unwrap().as_slice());
        assert_eq!(decoded, source.as_slice());

        let limited = HexOptions::lower(EncodingLimits::create(2, 3).unwrap());
        let mut limited_encoder = limited.encoder().unwrap();
        assert_eq!(
            limited_encoder.push(&bytes(b"ab")).unwrap_err().kind,
            EncodingErrorKind::ResourceLimit
        );
    }

    #[test]
    fn reader_and_writer_paths_preserve_stream_semantics() {
        let options = Base64Options::standard(EncodingLimits::default());
        let mut reader = SliceReader::new(b"tondo".to_vec(), 1).unwrap();
        let encoded = options.encode_to_reader_for_test(&mut reader).unwrap();
        assert_eq!(encoded.as_slice(), b"dG9uZG8=");

        let mut writer = VecWriter::with_max_write(2).unwrap();
        options
            .encode_to(&bytes(b"tondo"), &mut writer)
            .expect("short writes are accepted");
        assert_eq!(writer.bytes(), b"dG9uZG8=");
        assert!(writer.flushed());

        let mut source = SliceReader::new(b"dG9uZG8=".to_vec(), 2).unwrap();
        assert_eq!(
            options.decode_from(&mut source).unwrap().as_slice(),
            b"tondo"
        );
    }

    #[test]
    fn writer_no_progress_and_io_errors_are_terminal() {
        struct Stalled;
        impl Writer for Stalled {
            fn write(&mut self, _: &[u8]) -> Result<usize, io::IoError> {
                Ok(0)
            }

            fn flush(&mut self) -> Result<(), io::IoError> {
                Ok(())
            }
        }
        let options = HexOptions::lower(EncodingLimits::default());
        let mut writer = Stalled;
        assert_eq!(
            options
                .encode_to(&bytes(b"x"), &mut writer)
                .unwrap_err()
                .kind,
            EncodingErrorKind::NoProgress
        );

        struct Broken;
        impl Writer for Broken {
            fn write(&mut self, _: &[u8]) -> Result<usize, io::IoError> {
                Err(io::IoError::Host)
            }

            fn flush(&mut self) -> Result<(), io::IoError> {
                Ok(())
            }
        }
        let mut writer = Broken;
        assert_eq!(
            options
                .encode_to(&bytes(b"x"), &mut writer)
                .unwrap_err()
                .kind,
            EncodingErrorKind::Io(io::IoError::Host)
        );
    }

    trait ReaderTestExt {
        fn encode_to_reader_for_test(
            self,
            reader: &mut impl Reader,
        ) -> Result<Bytes, EncodingError>;
    }

    impl ReaderTestExt for Base64Options {
        fn encode_to_reader_for_test(
            self,
            reader: &mut impl Reader,
        ) -> Result<Bytes, EncodingError> {
            let mut input = Vec::new();
            loop {
                match reader.read(64) {
                    Ok(ReadResult::Eof) => break,
                    Ok(ReadResult::Data(chunk)) => input.extend_from_slice(&chunk),
                    Err(error) => {
                        return Err(EncodingError::new(
                            EncodingErrorKind::Io(error),
                            input.len(),
                        ));
                    }
                }
            }
            self.encode(&Bytes::new(input))
        }
    }
}
