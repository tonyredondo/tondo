//! Bounded CBOR data model and scalar wire codec (RFC 8949).
//!
//! This is a Rust standard-library kernel. It does not register a Tondo
//! compiler intrinsic, VM host function, native ABI, or AOT lowering.

use crate::serialization::{
    self, Cbor, Decode, Decoder, Encode, Encoder, Event, Raw, SerializationError,
};

pub type CborRaw = Raw<Cbor>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CborFloat16 {
    pub bits: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborEntry {
    pub key: CborValue,
    pub value: CborValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborTag {
    pub number: u64,
    pub value: Box<CborValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborValue {
    Null,
    Undefined,
    Bool(bool),
    Simple(u8),
    UInt(u64),
    /// The wire magnitude represents `-1 - magnitude`, including `-2^64`.
    Negative(u64),
    Float16(CborFloat16),
    Float32(u32),
    Float64(u64),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<CborValue>),
    Map(Vec<CborEntry>),
    Tag(CborTag),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CborDuplicatePolicy {
    Preserve,
    Reject,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CborUnknownTagPolicy {
    Preserve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CborNonMinimalPolicy {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CborIndefinitePolicy {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CborLimits {
    pub max_document_bytes: usize,
    pub max_depth: usize,
    pub max_array_items: usize,
    pub max_map_pairs: usize,
    pub max_string_bytes: usize,
    pub max_byte_string_bytes: usize,
    pub max_chunks: usize,
    pub max_tags: usize,
    pub max_simple_values: usize,
    pub max_events: usize,
    pub max_output_bytes: usize,
}

impl Default for CborLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_array_items: 1_048_576,
            max_map_pairs: 1_048_576,
            max_string_bytes: 64 * 1024 * 1024,
            max_byte_string_bytes: 64 * 1024 * 1024,
            max_chunks: 1_048_576,
            max_tags: 4096,
            max_simple_values: 1_048_576,
            max_events: 2_097_152,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

impl CborLimits {
    pub fn create(values: Self) -> Result<Self, CborError> {
        let all = [
            values.max_document_bytes,
            values.max_depth,
            values.max_array_items,
            values.max_map_pairs,
            values.max_string_bytes,
            values.max_byte_string_bytes,
            values.max_chunks,
            values.max_tags,
            values.max_simple_values,
            values.max_events,
            values.max_output_bytes,
        ];
        if all.contains(&0) || all.iter().any(|&n| n > isize::MAX as usize) {
            return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
        }
        Ok(values)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CborDecodeOptions {
    pub limits: CborLimits,
    pub dynamic_map_duplicates: CborDuplicatePolicy,
    pub typed_map_duplicates: CborDuplicatePolicy,
    pub unknown_tags: CborUnknownTagPolicy,
    pub non_minimal: CborNonMinimalPolicy,
    pub indefinite: CborIndefinitePolicy,
}

impl Default for CborDecodeOptions {
    fn default() -> Self {
        Self {
            limits: CborLimits::default(),
            dynamic_map_duplicates: CborDuplicatePolicy::Preserve,
            typed_map_duplicates: CborDuplicatePolicy::Reject,
            unknown_tags: CborUnknownTagPolicy::Preserve,
            non_minimal: CborNonMinimalPolicy::Accept,
            indefinite: CborIndefinitePolicy::Accept,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CborEncodeOptions {
    pub limits: CborLimits,
    pub deterministic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborPath {
    ArrayIndex(usize),
    MapEntry(usize),
    MapKey,
    MapValue,
    Tag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CborErrorKind {
    UnexpectedEof,
    InvalidInitialByte,
    InvalidAdditionalInfo,
    InvalidBreak,
    InvalidUtf8,
    InvalidSimpleValue,
    InvalidFloat,
    InvalidLength,
    NonMinimalEncoding,
    IndefiniteNotAllowed,
    InvalidChunk,
    InvalidTag,
    UnknownTag,
    TypeMismatch,
    DuplicateKey,
    DeterministicKeyCollision,
    OutOfOrderKey,
    NumberRange,
    LimitExceeded,
    TooManyChunks,
    TooManyTags,
    TrailingData,
    IoError,
    Closed,
    NoProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborError {
    pub kind: CborErrorKind,
    pub start_offset: usize,
    pub end_offset: usize,
    pub path: Vec<CborPath>,
}

impl CborError {
    fn at(kind: CborErrorKind, start_offset: usize, end_offset: usize) -> Self {
        Self {
            kind,
            start_offset,
            end_offset,
            path: Vec::new(),
        }
    }
}

impl std::fmt::Display for CborError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CBOR {:?} at bytes {}..{}",
            self.kind, self.start_offset, self.end_offset
        )
    }
}

impl std::error::Error for CborError {}

impl From<SerializationError> for CborError {
    fn from(error: SerializationError) -> Self {
        let kind = match error {
            SerializationError::LimitExceeded => CborErrorKind::LimitExceeded,
            SerializationError::EndOfInput => CborErrorKind::UnexpectedEof,
            SerializationError::DuplicateField => CborErrorKind::DuplicateKey,
            SerializationError::InvalidContainerLength
            | SerializationError::UnbalancedContainer => CborErrorKind::InvalidLength,
            _ => CborErrorKind::TypeMismatch,
        };
        Self::at(kind, 0, 0)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CborValueView<'a> {
    input: &'a [u8],
    options: CborDecodeOptions,
}

impl CborValueView<'_> {
    pub fn as_bytes(&self) -> &[u8] {
        self.input
    }
    pub fn own(&self) -> Result<CborValue, CborError> {
        parse(self.input, self.options)
    }
}

pub fn parse_view(
    input: &[u8],
    options: CborDecodeOptions,
) -> Result<CborValueView<'_>, CborError> {
    validate(input, options)?;
    Ok(CborValueView { input, options })
}

pub fn validate(input: &[u8], options: CborDecodeOptions) -> Result<(), CborError> {
    parse(input, options).map(|_| ())
}

pub fn raw(input: &[u8], options: CborDecodeOptions) -> Result<CborRaw, CborError> {
    validate(input, options)?;
    Ok(CborRaw::from_validated(input.to_vec()))
}

pub fn raw_unchecked(input: Vec<u8>) -> CborRaw {
    CborRaw::from_unchecked(input)
}

struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
    options: CborDecodeOptions,
    tags: usize,
    simple_values: usize,
    events: usize,
    path: Vec<CborPath>,
}

impl Cursor<'_> {
    fn error(&self, kind: CborErrorKind, start: usize) -> CborError {
        CborError {
            kind,
            start_offset: start,
            end_offset: self.pos,
            path: self.path.clone(),
        }
    }

    fn take(&mut self, count: usize) -> Result<&[u8], CborError> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or_else(|| self.error(CborErrorKind::InvalidLength, self.pos))?;
        if end > self.input.len() {
            return Err(self.error(CborErrorKind::UnexpectedEof, self.pos));
        }
        let bytes = &self.input[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn argument(&mut self, ai: u8, start: usize) -> Result<Option<u64>, CborError> {
        let value = match ai {
            0..=23 => return Ok(Some(u64::from(ai))),
            24 => u64::from(self.take(1)?[0]),
            25 => u64::from(u16::from_be_bytes(self.take(2)?.try_into().unwrap())),
            26 => u64::from(u32::from_be_bytes(self.take(4)?.try_into().unwrap())),
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Ok(None),
            _ => return Err(self.error(CborErrorKind::InvalidAdditionalInfo, start)),
        };
        if self.options.non_minimal == CborNonMinimalPolicy::Reject
            && ((ai == 24 && value < 24)
                || (ai == 25 && value <= u8::MAX.into())
                || (ai == 26 && value <= u16::MAX.into())
                || (ai == 27 && value <= u32::MAX.into()))
        {
            return Err(self.error(CborErrorKind::NonMinimalEncoding, start));
        }
        Ok(Some(value))
    }

    fn length(&mut self, ai: u8, start: usize, limit: usize) -> Result<Option<usize>, CborError> {
        match self.argument(ai, start)? {
            Some(n) => {
                let n = usize::try_from(n)
                    .map_err(|_| self.error(CborErrorKind::InvalidLength, start))?;
                if n > limit {
                    return Err(self.error(CborErrorKind::LimitExceeded, start));
                }
                Ok(Some(n))
            }
            None if self.options.indefinite == CborIndefinitePolicy::Accept => Ok(None),
            None => Err(self.error(CborErrorKind::IndefiniteNotAllowed, start)),
        }
    }

    fn indefinite_string(
        &mut self,
        major: u8,
        limit: usize,
        start: usize,
    ) -> Result<Vec<u8>, CborError> {
        let mut result = Vec::new();
        let mut chunks = 0usize;
        loop {
            let begin = self.pos;
            let head = *self.take(1)?.first().unwrap();
            if head == 0xff {
                break;
            }
            if head >> 5 != major || head & 31 == 31 {
                return Err(self.error(CborErrorKind::InvalidChunk, begin));
            }
            chunks = chunks
                .checked_add(1)
                .ok_or_else(|| self.error(CborErrorKind::TooManyChunks, begin))?;
            if chunks > self.options.limits.max_chunks {
                return Err(self.error(CborErrorKind::TooManyChunks, begin));
            }
            let remaining = limit - result.len();
            let size = self
                .length(head & 31, begin, remaining)?
                .ok_or_else(|| self.error(CborErrorKind::InvalidChunk, begin))?;
            let part = self.take(size)?;
            if major == 3 && std::str::from_utf8(part).is_err() {
                return Err(self.error(CborErrorKind::InvalidUtf8, begin));
            }
            result.extend_from_slice(part);
        }
        let _ = start;
        Ok(result)
    }
}

enum ParseFrame {
    Array {
        items: Vec<CborValue>,
        remaining: Option<usize>,
    },
    Map {
        entries: Vec<CborEntry>,
        key: Option<CborValue>,
        remaining: Option<usize>,
        pairs: usize,
    },
    Tag(u64),
}

pub fn parse(input: &[u8], options: CborDecodeOptions) -> Result<CborValue, CborError> {
    CborLimits::create(options.limits)?;
    if input.len() > options.limits.max_document_bytes {
        return Err(CborError::at(CborErrorKind::LimitExceeded, 0, input.len()));
    }
    let mut c = Cursor {
        input,
        pos: 0,
        options,
        tags: 0,
        simple_values: 0,
        events: 0,
        path: Vec::new(),
    };
    let mut stack: Vec<ParseFrame> = Vec::new();
    let mut current: Option<CborValue> = None;
    loop {
        if let Some(value) = current.take() {
            match stack.last_mut() {
                Some(ParseFrame::Array { items, remaining }) => {
                    items.push(value);
                    if let Some(left) = remaining {
                        *left -= 1;
                    }
                    continue;
                }
                Some(ParseFrame::Map {
                    entries,
                    key,
                    remaining,
                    pairs,
                }) => {
                    if let Some(saved_key) = key.take() {
                        let index = entries.iter().position(|entry| entry.key == saved_key);
                        match (index, options.dynamic_map_duplicates) {
                            (Some(_), CborDuplicatePolicy::Reject) => {
                                return Err(c.error(CborErrorKind::DuplicateKey, c.pos));
                            }
                            (Some(_), CborDuplicatePolicy::First) => {}
                            (Some(i), CborDuplicatePolicy::Last) => entries[i].value = value,
                            _ => entries.push(CborEntry {
                                key: saved_key,
                                value,
                            }),
                        }
                        if let Some(left) = remaining {
                            *left -= 1;
                        }
                        *pairs += 1;
                    } else {
                        *key = Some(value);
                    }
                    continue;
                }
                Some(ParseFrame::Tag(_)) => {
                    let Some(ParseFrame::Tag(number)) = stack.pop() else {
                        unreachable!()
                    };
                    current = Some(CborValue::Tag(CborTag {
                        number,
                        value: Box::new(value),
                    }));
                    continue;
                }
                None => {
                    if c.pos != input.len() {
                        return Err(c.error(CborErrorKind::TrailingData, c.pos));
                    }
                    return Ok(value);
                }
            }
        }

        c.path.clear();
        for frame in &stack {
            match frame {
                ParseFrame::Array { items, .. } => c.path.push(CborPath::ArrayIndex(items.len())),
                ParseFrame::Map { pairs, key, .. } => {
                    c.path.push(CborPath::MapEntry(*pairs));
                    c.path.push(if key.is_some() {
                        CborPath::MapValue
                    } else {
                        CborPath::MapKey
                    });
                }
                ParseFrame::Tag(_) => c.path.push(CborPath::Tag),
            }
        }

        let close = match stack.last() {
            Some(ParseFrame::Array {
                remaining: Some(0), ..
            }) => true,
            Some(ParseFrame::Map {
                remaining: Some(0),
                key: None,
                ..
            }) => true,
            Some(ParseFrame::Array {
                remaining: None, ..
            })
            | Some(ParseFrame::Map {
                remaining: None,
                key: None,
                ..
            }) => c.input.get(c.pos) == Some(&0xff),
            _ => false,
        };
        if close {
            let frame = stack.pop().unwrap();
            match frame {
                ParseFrame::Array { items, remaining } => {
                    if remaining.is_none() {
                        c.pos += 1;
                    }
                    current = Some(CborValue::Array(items));
                }
                ParseFrame::Map {
                    entries, remaining, ..
                } => {
                    if remaining.is_none() {
                        c.pos += 1;
                    }
                    current = Some(CborValue::Map(entries));
                }
                ParseFrame::Tag(_) => unreachable!(),
            }
            continue;
        }
        if c.pos >= input.len() {
            return Err(c.error(CborErrorKind::UnexpectedEof, c.pos));
        }
        if stack.len() >= options.limits.max_depth {
            return Err(c.error(CborErrorKind::LimitExceeded, c.pos));
        }
        if let Some(frame) = stack.last() {
            match frame {
                ParseFrame::Array { items, .. }
                    if items.len() >= options.limits.max_array_items =>
                {
                    return Err(c.error(CborErrorKind::LimitExceeded, c.pos));
                }
                ParseFrame::Map {
                    pairs, key: None, ..
                } if *pairs >= options.limits.max_map_pairs => {
                    return Err(c.error(CborErrorKind::LimitExceeded, c.pos));
                }
                _ => {}
            }
        }
        c.events += 1;
        if c.events > options.limits.max_events {
            return Err(c.error(CborErrorKind::LimitExceeded, c.pos));
        }
        let start = c.pos;
        let head = c.take(1)?[0];
        let major = head >> 5;
        let ai = head & 31;
        current = match major {
            0 => Some(CborValue::UInt(
                c.argument(ai, start)?
                    .ok_or_else(|| c.error(CborErrorKind::InvalidLength, start))?,
            )),
            1 => Some(CborValue::Negative(
                c.argument(ai, start)?
                    .ok_or_else(|| c.error(CborErrorKind::InvalidLength, start))?,
            )),
            2 | 3 => {
                let limit = if major == 2 {
                    options.limits.max_byte_string_bytes
                } else {
                    options.limits.max_string_bytes
                };
                let bytes = match c.length(ai, start, limit)? {
                    Some(size) => c.take(size)?.to_vec(),
                    None => c.indefinite_string(major, limit, start)?,
                };
                if major == 2 {
                    Some(CborValue::Bytes(bytes))
                } else {
                    Some(CborValue::Text(
                        String::from_utf8(bytes)
                            .map_err(|_| c.error(CborErrorKind::InvalidUtf8, start))?,
                    ))
                }
            }
            4 | 5 => {
                let limit = if major == 4 {
                    options.limits.max_array_items
                } else {
                    options.limits.max_map_pairs
                };
                let remaining = c.length(ai, start, limit)?;
                if major == 4 {
                    stack.push(ParseFrame::Array {
                        items: Vec::new(),
                        remaining,
                    });
                } else {
                    stack.push(ParseFrame::Map {
                        entries: Vec::new(),
                        key: None,
                        remaining,
                        pairs: 0,
                    });
                }
                None
            }
            6 => {
                let number = c
                    .argument(ai, start)?
                    .ok_or_else(|| c.error(CborErrorKind::InvalidTag, start))?;
                c.tags += 1;
                if c.tags > options.limits.max_tags {
                    return Err(c.error(CborErrorKind::TooManyTags, start));
                }
                if options.unknown_tags == CborUnknownTagPolicy::Reject {
                    return Err(c.error(CborErrorKind::UnknownTag, start));
                }
                stack.push(ParseFrame::Tag(number));
                None
            }
            7 => match ai {
                0..=19 => Some(CborValue::Simple(ai)),
                20 => Some(CborValue::Bool(false)),
                21 => Some(CborValue::Bool(true)),
                22 => Some(CborValue::Null),
                23 => Some(CborValue::Undefined),
                24 => {
                    let simple = c.take(1)?[0];
                    match simple {
                        0..=19 if options.non_minimal == CborNonMinimalPolicy::Reject => {
                            return Err(c.error(CborErrorKind::NonMinimalEncoding, start));
                        }
                        0..=19 => Some(CborValue::Simple(simple)),
                        20..=23 => return Err(c.error(CborErrorKind::InvalidSimpleValue, start)),
                        24..=31 => return Err(c.error(CborErrorKind::InvalidSimpleValue, start)),
                        _ => Some(CborValue::Simple(simple)),
                    }
                }
                25 => Some(CborValue::Float16(CborFloat16 {
                    bits: u16::from_be_bytes(c.take(2)?.try_into().unwrap()),
                })),
                26 => Some(CborValue::Float32(u32::from_be_bytes(
                    c.take(4)?.try_into().unwrap(),
                ))),
                27 => Some(CborValue::Float64(u64::from_be_bytes(
                    c.take(8)?.try_into().unwrap(),
                ))),
                31 => return Err(c.error(CborErrorKind::InvalidBreak, start)),
                _ => return Err(c.error(CborErrorKind::InvalidAdditionalInfo, start)),
            },
            _ => return Err(c.error(CborErrorKind::InvalidInitialByte, start)),
        };
        if options.non_minimal == CborNonMinimalPolicy::Reject
            && let Some(
                value @ (CborValue::Float16(_) | CborValue::Float32(_) | CborValue::Float64(_)),
            ) = &current
            && !preferred_float_wire(value, &input[start..c.pos], options.limits)
        {
            return Err(c.error(CborErrorKind::NonMinimalEncoding, start));
        }
        if matches!(current, Some(CborValue::Simple(_))) {
            c.simple_values += 1;
            if c.simple_values > options.limits.max_simple_values {
                return Err(c.error(CborErrorKind::LimitExceeded, start));
            }
        }
    }
}

fn push_bytes(out: &mut Vec<u8>, bytes: &[u8], limits: CborLimits) -> Result<(), CborError> {
    let end = out
        .len()
        .checked_add(bytes.len())
        .ok_or_else(|| CborError::at(CborErrorKind::LimitExceeded, out.len(), out.len()))?;
    if end > limits.max_output_bytes {
        return Err(CborError::at(
            CborErrorKind::LimitExceeded,
            out.len(),
            out.len(),
        ));
    }
    out.extend_from_slice(bytes);
    Ok(())
}

fn head(major: u8, n: u64) -> ([u8; 9], usize) {
    let mut bytes = [0u8; 9];
    if n < 24 {
        bytes[0] = major << 5 | n as u8;
        (bytes, 1)
    } else if n <= u8::MAX.into() {
        bytes[0] = major << 5 | 24;
        bytes[1] = n as u8;
        (bytes, 2)
    } else if n <= u16::MAX.into() {
        bytes[0] = major << 5 | 25;
        bytes[1..3].copy_from_slice(&(n as u16).to_be_bytes());
        (bytes, 3)
    } else if n <= u32::MAX.into() {
        bytes[0] = major << 5 | 26;
        bytes[1..5].copy_from_slice(&(n as u32).to_be_bytes());
        (bytes, 5)
    } else {
        bytes[0] = major << 5 | 27;
        bytes[1..9].copy_from_slice(&n.to_be_bytes());
        (bytes, 9)
    }
}

fn push_head(out: &mut Vec<u8>, major: u8, n: u64, limits: CborLimits) -> Result<(), CborError> {
    let (bytes, len) = head(major, n);
    push_bytes(out, &bytes[..len], limits)
}

fn half_to_f32(bits: u16) -> f32 {
    let sign = u32::from(bits & 0x8000) << 16;
    let exp = (bits >> 10) & 31;
    let fraction = bits & 0x03ff;
    let result = match exp {
        0 if fraction == 0 => sign,
        0 => {
            let mut mantissa = u32::from(fraction);
            let mut exponent = -14i32;
            while mantissa & 0x400 == 0 {
                mantissa <<= 1;
                exponent -= 1;
            }
            sign | (((exponent + 127) as u32) << 23) | ((mantissa & 0x3ff) << 13)
        }
        31 => sign | 0x7f80_0000 | (u32::from(fraction) << 13),
        _ => sign | (u32::from(exp) + 112) << 23 | (u32::from(fraction) << 13),
    };
    f32::from_bits(result)
}

fn exact_half(value: f32) -> Option<u16> {
    let bits = value.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exp == 255 {
        return if mantissa == 0 {
            Some(sign | 0x7c00)
        } else {
            None
        };
    }
    if exp == 0 && mantissa == 0 {
        return Some(sign);
    }
    let unbiased = exp - 127;
    let candidate = if (-14..=15).contains(&unbiased) {
        if mantissa & 0x1fff != 0 {
            return None;
        }
        sign | (((unbiased + 15) as u16) << 10) | ((mantissa >> 13) as u16)
    } else if (-24..-14).contains(&unbiased) {
        let shift = (13 - 14 - unbiased) as u32;
        let fraction = (1 << 23) | mantissa;
        if fraction & ((1 << shift) - 1) != 0 {
            return None;
        }
        sign | ((fraction >> shift) as u16)
    } else {
        return None;
    };
    (half_to_f32(candidate).to_bits() == bits).then_some(candidate)
}

fn push_float(
    out: &mut Vec<u8>,
    value: &CborValue,
    deterministic: bool,
    limits: CborLimits,
) -> Result<(), CborError> {
    if deterministic {
        let numeric = match value {
            CborValue::Float16(v) => f64::from(half_to_f32(v.bits)),
            CborValue::Float32(bits) => f64::from(f32::from_bits(*bits)),
            CborValue::Float64(bits) => f64::from_bits(*bits),
            _ => unreachable!(),
        };
        if numeric.is_nan() {
            return push_bytes(out, &[0xf9, 0x7e, 0x00], limits);
        }
        let narrowed = numeric as f32;
        if f64::from(narrowed).to_bits() == numeric.to_bits() {
            if let Some(half) = exact_half(narrowed) {
                push_bytes(out, &[0xf9], limits)?;
                return push_bytes(out, &half.to_be_bytes(), limits);
            }
            push_bytes(out, &[0xfa], limits)?;
            return push_bytes(out, &narrowed.to_bits().to_be_bytes(), limits);
        }
        push_bytes(out, &[0xfb], limits)?;
        return push_bytes(out, &numeric.to_bits().to_be_bytes(), limits);
    }
    match value {
        CborValue::Float16(v) => {
            push_bytes(out, &[0xf9], limits)?;
            push_bytes(out, &v.bits.to_be_bytes(), limits)
        }
        CborValue::Float32(bits) => {
            push_bytes(out, &[0xfa], limits)?;
            push_bytes(out, &bits.to_be_bytes(), limits)
        }
        CborValue::Float64(bits) => {
            push_bytes(out, &[0xfb], limits)?;
            push_bytes(out, &bits.to_be_bytes(), limits)
        }
        _ => unreachable!(),
    }
}

fn preferred_float_wire(value: &CborValue, wire: &[u8], limits: CborLimits) -> bool {
    let mut preferred = Vec::new();
    push_float(
        &mut preferred,
        value,
        true,
        CborLimits {
            max_output_bytes: 9,
            ..limits
        },
    )
    .is_ok()
        && preferred == wire
}

fn check_value(value: &CborValue, limits: CborLimits) -> Result<(), CborError> {
    let mut work = vec![(value, 0usize)];
    let mut events = 0usize;
    let mut tags = 0usize;
    let mut simples = 0usize;
    while let Some((value, depth)) = work.pop() {
        events += 1;
        if events > limits.max_events || depth > limits.max_depth {
            return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
        }
        match value {
            CborValue::Array(items) => {
                if items.len() > limits.max_array_items {
                    return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
                }
                for item in items {
                    work.push((item, depth + 1));
                }
            }
            CborValue::Map(entries) => {
                if entries.len() > limits.max_map_pairs {
                    return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
                }
                for entry in entries {
                    work.push((&entry.key, depth + 1));
                    work.push((&entry.value, depth + 1));
                }
            }
            CborValue::Tag(tag) => {
                tags += 1;
                work.push((&tag.value, depth + 1));
            }
            CborValue::Simple(_) => simples += 1,
            CborValue::Text(value) if value.len() > limits.max_string_bytes => {
                return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
            }
            CborValue::Bytes(value) if value.len() > limits.max_byte_string_bytes => {
                return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
            }
            _ => {}
        }
        if tags > limits.max_tags {
            return Err(CborError::at(CborErrorKind::TooManyTags, 0, 0));
        }
        if simples > limits.max_simple_values {
            return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
        }
    }
    Ok(())
}

enum EncodeWork<'a> {
    Value(&'a CborValue, usize),
}

fn encode_inner(value: &CborValue, options: CborEncodeOptions) -> Result<Vec<u8>, CborError> {
    let mut out = Vec::new();
    let mut stack = vec![EncodeWork::Value(value, 0)];
    let mut events = 0usize;
    let mut tags = 0usize;
    let mut simples = 0usize;
    while let Some(work) = stack.pop() {
        let EncodeWork::Value(value, depth) = work;
        events += 1;
        if events > options.limits.max_events || depth > options.limits.max_depth {
            return Err(CborError::at(
                CborErrorKind::LimitExceeded,
                out.len(),
                out.len(),
            ));
        }
        match value {
            CborValue::Null => push_bytes(&mut out, &[0xf6], options.limits)?,
            CborValue::Undefined => push_bytes(&mut out, &[0xf7], options.limits)?,
            CborValue::Bool(false) => push_bytes(&mut out, &[0xf4], options.limits)?,
            CborValue::Bool(true) => push_bytes(&mut out, &[0xf5], options.limits)?,
            CborValue::Simple(n) => {
                simples += 1;
                if simples > options.limits.max_simple_values {
                    return Err(CborError::at(
                        CborErrorKind::LimitExceeded,
                        out.len(),
                        out.len(),
                    ));
                }
                if (20..=31).contains(n) {
                    return Err(CborError::at(
                        CborErrorKind::InvalidSimpleValue,
                        out.len(),
                        out.len(),
                    ));
                }
                if *n < 20 {
                    push_bytes(&mut out, &[0xe0 | n], options.limits)?;
                } else {
                    push_bytes(&mut out, &[0xf8, *n], options.limits)?;
                }
            }
            CborValue::UInt(n) => push_head(&mut out, 0, *n, options.limits)?,
            CborValue::Negative(n) => push_head(&mut out, 1, *n, options.limits)?,
            CborValue::Float16(_) | CborValue::Float32(_) | CborValue::Float64(_) => {
                push_float(&mut out, value, options.deterministic, options.limits)?
            }
            CborValue::Bytes(bytes) => {
                if bytes.len() > options.limits.max_byte_string_bytes {
                    return Err(CborError::at(
                        CborErrorKind::LimitExceeded,
                        out.len(),
                        out.len(),
                    ));
                }
                push_head(&mut out, 2, bytes.len() as u64, options.limits)?;
                push_bytes(&mut out, bytes, options.limits)?;
            }
            CborValue::Text(text) => {
                if text.len() > options.limits.max_string_bytes {
                    return Err(CborError::at(
                        CborErrorKind::LimitExceeded,
                        out.len(),
                        out.len(),
                    ));
                }
                push_head(&mut out, 3, text.len() as u64, options.limits)?;
                push_bytes(&mut out, text.as_bytes(), options.limits)?;
            }
            CborValue::Array(items) => {
                if items.len() > options.limits.max_array_items {
                    return Err(CborError::at(
                        CborErrorKind::LimitExceeded,
                        out.len(),
                        out.len(),
                    ));
                }
                push_head(&mut out, 4, items.len() as u64, options.limits)?;
                for item in items.iter().rev() {
                    stack.push(EncodeWork::Value(item, depth + 1));
                }
            }
            CborValue::Map(entries) => {
                if entries.len() > options.limits.max_map_pairs {
                    return Err(CborError::at(
                        CborErrorKind::LimitExceeded,
                        out.len(),
                        out.len(),
                    ));
                }
                push_head(&mut out, 5, entries.len() as u64, options.limits)?;
                for entry in entries.iter().rev() {
                    stack.push(EncodeWork::Value(&entry.value, depth + 1));
                    stack.push(EncodeWork::Value(&entry.key, depth + 1));
                }
            }
            CborValue::Tag(tag) => {
                tags += 1;
                if tags > options.limits.max_tags {
                    return Err(CborError::at(
                        CborErrorKind::TooManyTags,
                        out.len(),
                        out.len(),
                    ));
                }
                push_head(&mut out, 6, tag.number, options.limits)?;
                stack.push(EncodeWork::Value(&tag.value, depth + 1));
            }
        }
    }
    Ok(out)
}

enum DeterministicWork<'a> {
    Value(&'a CborValue, usize),
    Array(usize),
    Map(usize),
    Tag(u64),
}

fn encode_deterministic_inner(value: &CborValue, limits: CborLimits) -> Result<Vec<u8>, CborError> {
    let mut work = vec![DeterministicWork::Value(value, 0)];
    let mut finished: Vec<Vec<u8>> = Vec::new();
    while let Some(task) = work.pop() {
        match task {
            DeterministicWork::Value(CborValue::Array(items), depth) => {
                work.push(DeterministicWork::Array(items.len()));
                for item in items.iter().rev() {
                    work.push(DeterministicWork::Value(item, depth + 1));
                }
            }
            DeterministicWork::Value(CborValue::Map(entries), depth) => {
                work.push(DeterministicWork::Map(entries.len()));
                for entry in entries.iter().rev() {
                    work.push(DeterministicWork::Value(&entry.value, depth + 1));
                    work.push(DeterministicWork::Value(&entry.key, depth + 1));
                }
            }
            DeterministicWork::Value(CborValue::Tag(tag), depth) => {
                work.push(DeterministicWork::Tag(tag.number));
                work.push(DeterministicWork::Value(&tag.value, depth + 1));
            }
            DeterministicWork::Value(value, _) => {
                let mut out = Vec::new();
                match value {
                    CborValue::Null => push_bytes(&mut out, &[0xf6], limits)?,
                    CborValue::Undefined => push_bytes(&mut out, &[0xf7], limits)?,
                    CborValue::Bool(false) => push_bytes(&mut out, &[0xf4], limits)?,
                    CborValue::Bool(true) => push_bytes(&mut out, &[0xf5], limits)?,
                    CborValue::Simple(n) if *n < 20 => push_bytes(&mut out, &[0xe0 | n], limits)?,
                    CborValue::Simple(n) if *n >= 32 => push_bytes(&mut out, &[0xf8, *n], limits)?,
                    CborValue::Simple(_) => {
                        return Err(CborError::at(CborErrorKind::InvalidSimpleValue, 0, 0));
                    }
                    CborValue::UInt(n) => push_head(&mut out, 0, *n, limits)?,
                    CborValue::Negative(n) => push_head(&mut out, 1, *n, limits)?,
                    CborValue::Float16(_) | CborValue::Float32(_) | CborValue::Float64(_) => {
                        push_float(&mut out, value, true, limits)?
                    }
                    CborValue::Bytes(bytes) => {
                        push_head(&mut out, 2, bytes.len() as u64, limits)?;
                        push_bytes(&mut out, bytes, limits)?;
                    }
                    CborValue::Text(text) => {
                        push_head(&mut out, 3, text.len() as u64, limits)?;
                        push_bytes(&mut out, text.as_bytes(), limits)?;
                    }
                    _ => unreachable!(),
                }
                finished.push(out);
            }
            DeterministicWork::Array(count) => {
                let start = finished.len() - count;
                let children = finished.split_off(start);
                let mut out = Vec::new();
                push_head(&mut out, 4, count as u64, limits)?;
                for child in children {
                    push_bytes(&mut out, &child, limits)?;
                }
                finished.push(out);
            }
            DeterministicWork::Map(count) => {
                let start = finished.len() - count * 2;
                let parts = finished.split_off(start);
                let mut pairs = Vec::with_capacity(count);
                let mut parts = parts.into_iter();
                while let Some(key) = parts.next() {
                    pairs.push((key, parts.next().unwrap()));
                }
                pairs.sort_by(|left, right| left.0.cmp(&right.0));
                if pairs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                    return Err(CborError::at(
                        CborErrorKind::DeterministicKeyCollision,
                        0,
                        0,
                    ));
                }
                let mut out = Vec::new();
                push_head(&mut out, 5, count as u64, limits)?;
                for (key, value) in pairs {
                    push_bytes(&mut out, &key, limits)?;
                    push_bytes(&mut out, &value, limits)?;
                }
                finished.push(out);
            }
            DeterministicWork::Tag(number) => {
                let child = finished.pop().unwrap();
                let mut out = Vec::new();
                push_head(&mut out, 6, number, limits)?;
                push_bytes(&mut out, &child, limits)?;
                finished.push(out);
            }
        }
        let mut live_bytes = 0usize;
        for bytes in &finished {
            live_bytes = live_bytes
                .checked_add(bytes.len())
                .ok_or_else(|| CborError::at(CborErrorKind::LimitExceeded, 0, 0))?;
            if live_bytes > limits.max_output_bytes {
                return Err(CborError::at(CborErrorKind::LimitExceeded, 0, 0));
            }
        }
    }
    Ok(finished.pop().unwrap())
}

pub fn encode(value: &CborValue, options: CborEncodeOptions) -> Result<Vec<u8>, CborError> {
    CborLimits::create(options.limits)?;
    check_value(value, options.limits)?;
    if options.deterministic {
        encode_deterministic_inner(value, options.limits)
    } else {
        encode_inner(value, options)
    }
}

pub fn encode_deterministic(value: &CborValue, limits: CborLimits) -> Result<Vec<u8>, CborError> {
    encode(
        value,
        CborEncodeOptions {
            limits,
            deterministic: true,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborEvent {
    StreamStart,
    Null,
    Undefined,
    Bool(bool),
    Simple(u8),
    UInt(u64),
    Negative(u64),
    Float16(CborFloat16),
    Float32(u32),
    Float64(u64),
    Bytes(Vec<u8>),
    Text(String),
    StartBytes(Option<usize>),
    ByteChunk(Vec<u8>),
    EndBytes,
    StartText(Option<usize>),
    TextChunk(String),
    EndText,
    StartArray(Option<usize>),
    EndArray,
    StartMap(Option<usize>),
    MapKey,
    EndMap,
    Tag(u64),
    StreamEnd,
}

enum ScanFrame {
    Array {
        remaining: Option<usize>,
        count: usize,
    },
    Map {
        remaining: Option<usize>,
        count: usize,
        stage: u8,
    },
    Tag,
}

fn scan_complete(stack: &mut Vec<ScanFrame>, root_done: &mut bool) {
    loop {
        match stack.last_mut() {
            Some(ScanFrame::Array { remaining, count }) => {
                *count += 1;
                if let Some(left) = remaining {
                    *left -= 1;
                }
                return;
            }
            Some(ScanFrame::Map { stage, .. }) if *stage == 1 => {
                *stage = 2;
                return;
            }
            Some(ScanFrame::Map {
                remaining,
                count,
                stage,
            }) if *stage == 3 => {
                *stage = 0;
                *count += 1;
                if let Some(left) = remaining {
                    *left -= 1;
                }
                return;
            }
            Some(ScanFrame::Tag) => {
                stack.pop();
            }
            None => {
                *root_done = true;
                return;
            }
            _ => unreachable!(),
        }
    }
}

fn scan_events_inner(
    input: &[u8],
    options: CborDecodeOptions,
    dynamic_policy: bool,
) -> Result<Vec<CborEvent>, CborError> {
    CborLimits::create(options.limits)?;
    if input.len() > options.limits.max_document_bytes {
        return Err(CborError::at(CborErrorKind::LimitExceeded, 0, input.len()));
    }
    // Dynamic policy may collapse map pairs, so the public event reader also
    // checks that policy before exposing any events. Typed decoding scans the
    // wire directly and leaves duplicate rejection to the typed map decoder.
    if dynamic_policy {
        parse(input, options)?;
    }
    let mut cursor = Cursor {
        input,
        pos: 0,
        options,
        tags: 0,
        simple_values: 0,
        events: 0,
        path: Vec::new(),
    };
    let mut events = vec![CborEvent::StreamStart];
    let mut stack = Vec::new();
    let mut root_done = false;
    while !root_done {
        let closing = match stack.last() {
            Some(ScanFrame::Array {
                remaining: Some(0), ..
            }) => Some(CborEvent::EndArray),
            Some(ScanFrame::Map {
                remaining: Some(0),
                stage: 0,
                ..
            }) => Some(CborEvent::EndMap),
            Some(ScanFrame::Array {
                remaining: None, ..
            }) if input.get(cursor.pos) == Some(&0xff) => Some(CborEvent::EndArray),
            Some(ScanFrame::Map {
                remaining: None,
                stage: 0,
                ..
            }) if input.get(cursor.pos) == Some(&0xff) => Some(CborEvent::EndMap),
            _ => None,
        };
        if let Some(event) = closing {
            let frame = stack.pop().unwrap();
            if matches!(
                frame,
                ScanFrame::Array {
                    remaining: None,
                    ..
                } | ScanFrame::Map {
                    remaining: None,
                    ..
                }
            ) {
                cursor.pos += 1;
            }
            events.push(event);
            scan_complete(&mut stack, &mut root_done);
            continue;
        }
        if let Some(ScanFrame::Map { stage, .. }) = stack.last_mut() {
            if *stage == 0 {
                events.push(CborEvent::MapKey);
                *stage = 1;
            } else if *stage == 2 {
                *stage = 3;
            }
        }
        if cursor.pos == input.len() {
            return Err(cursor.error(CborErrorKind::UnexpectedEof, cursor.pos));
        }
        if stack.len() > options.limits.max_depth {
            return Err(cursor.error(CborErrorKind::LimitExceeded, cursor.pos));
        }
        match stack.last() {
            Some(ScanFrame::Array { count, .. }) if *count >= options.limits.max_array_items => {
                return Err(cursor.error(CborErrorKind::LimitExceeded, cursor.pos));
            }
            Some(ScanFrame::Map {
                count, stage: 1, ..
            }) if *count >= options.limits.max_map_pairs => {
                return Err(cursor.error(CborErrorKind::LimitExceeded, cursor.pos));
            }
            _ => {}
        }
        let start = cursor.pos;
        let head = cursor.take(1)?[0];
        let major = head >> 5;
        let ai = head & 31;
        let mut complete = true;
        match major {
            0 => events.push(CborEvent::UInt(
                cursor
                    .argument(ai, start)?
                    .ok_or_else(|| cursor.error(CborErrorKind::InvalidLength, start))?,
            )),
            1 => events.push(CborEvent::Negative(
                cursor
                    .argument(ai, start)?
                    .ok_or_else(|| cursor.error(CborErrorKind::InvalidLength, start))?,
            )),
            2 | 3 => {
                let limit = if major == 2 {
                    options.limits.max_byte_string_bytes
                } else {
                    options.limits.max_string_bytes
                };
                match cursor.length(ai, start, limit)? {
                    Some(n) => {
                        let bytes = cursor.take(n)?.to_vec();
                        if major == 2 {
                            events.push(CborEvent::Bytes(bytes));
                        } else {
                            events
                                .push(CborEvent::Text(String::from_utf8(bytes).map_err(|_| {
                                    cursor.error(CborErrorKind::InvalidUtf8, start)
                                })?));
                        }
                    }
                    None => {
                        if major == 2 {
                            events.push(CborEvent::StartBytes(None));
                        } else {
                            events.push(CborEvent::StartText(None));
                        }
                        let mut chunks = 0usize;
                        let mut total = 0usize;
                        loop {
                            let start = cursor.pos;
                            let head = cursor.take(1)?[0];
                            if head == 0xff {
                                break;
                            }
                            if head >> 5 != major || head & 31 == 31 {
                                return Err(cursor.error(CborErrorKind::InvalidChunk, start));
                            }
                            chunks += 1;
                            if chunks > options.limits.max_chunks {
                                return Err(cursor.error(CborErrorKind::TooManyChunks, start));
                            }
                            let n = cursor
                                .length(head & 31, start, limit - total)?
                                .ok_or_else(|| cursor.error(CborErrorKind::InvalidChunk, start))?;
                            total += n;
                            let bytes = cursor.take(n)?.to_vec();
                            if major == 2 {
                                events.push(CborEvent::ByteChunk(bytes));
                            } else {
                                events.push(CborEvent::TextChunk(
                                    String::from_utf8(bytes).map_err(|_| {
                                        cursor.error(CborErrorKind::InvalidUtf8, start)
                                    })?,
                                ));
                            }
                            if events.len() + 2 > options.limits.max_events {
                                return Err(cursor.error(CborErrorKind::LimitExceeded, cursor.pos));
                            }
                        }
                        if major == 2 {
                            events.push(CborEvent::EndBytes);
                        } else {
                            events.push(CborEvent::EndText);
                        }
                    }
                }
            }
            4 | 5 => {
                let limit = if major == 4 {
                    options.limits.max_array_items
                } else {
                    options.limits.max_map_pairs
                };
                let remaining = cursor.length(ai, start, limit)?;
                if major == 4 {
                    events.push(CborEvent::StartArray(remaining));
                    stack.push(ScanFrame::Array {
                        remaining,
                        count: 0,
                    });
                } else {
                    events.push(CborEvent::StartMap(remaining));
                    stack.push(ScanFrame::Map {
                        remaining,
                        count: 0,
                        stage: 0,
                    });
                }
                if stack.len() > options.limits.max_depth {
                    return Err(cursor.error(CborErrorKind::LimitExceeded, start));
                }
                complete = false;
            }
            6 => {
                let number = cursor
                    .argument(ai, start)?
                    .ok_or_else(|| cursor.error(CborErrorKind::InvalidTag, start))?;
                cursor.tags += 1;
                if cursor.tags > options.limits.max_tags {
                    return Err(cursor.error(CborErrorKind::TooManyTags, start));
                }
                if options.unknown_tags == CborUnknownTagPolicy::Reject {
                    return Err(cursor.error(CborErrorKind::UnknownTag, start));
                }
                events.push(CborEvent::Tag(number));
                stack.push(ScanFrame::Tag);
                if stack.len() > options.limits.max_depth {
                    return Err(cursor.error(CborErrorKind::LimitExceeded, start));
                }
                complete = false;
            }
            7 => events.push(match ai {
                0..=19 => {
                    cursor.simple_values += 1;
                    if cursor.simple_values > options.limits.max_simple_values {
                        return Err(cursor.error(CborErrorKind::LimitExceeded, start));
                    }
                    CborEvent::Simple(ai)
                }
                20 => CborEvent::Bool(false),
                21 => CborEvent::Bool(true),
                22 => CborEvent::Null,
                23 => CborEvent::Undefined,
                24 => {
                    let n = cursor.take(1)?[0];
                    if (20..=31).contains(&n) {
                        return Err(cursor.error(CborErrorKind::InvalidSimpleValue, start));
                    }
                    if n < 20 && options.non_minimal == CborNonMinimalPolicy::Reject {
                        return Err(cursor.error(CborErrorKind::NonMinimalEncoding, start));
                    }
                    cursor.simple_values += 1;
                    if cursor.simple_values > options.limits.max_simple_values {
                        return Err(cursor.error(CborErrorKind::LimitExceeded, start));
                    }
                    CborEvent::Simple(n)
                }
                25 => CborEvent::Float16(CborFloat16 {
                    bits: u16::from_be_bytes(cursor.take(2)?.try_into().unwrap()),
                }),
                26 => CborEvent::Float32(u32::from_be_bytes(cursor.take(4)?.try_into().unwrap())),
                27 => CborEvent::Float64(u64::from_be_bytes(cursor.take(8)?.try_into().unwrap())),
                31 => return Err(cursor.error(CborErrorKind::InvalidBreak, start)),
                _ => return Err(cursor.error(CborErrorKind::InvalidAdditionalInfo, start)),
            }),
            _ => return Err(cursor.error(CborErrorKind::InvalidInitialByte, start)),
        }
        if options.non_minimal == CborNonMinimalPolicy::Reject {
            let float = match events.last() {
                Some(CborEvent::Float16(value)) => Some(CborValue::Float16(*value)),
                Some(CborEvent::Float32(value)) => Some(CborValue::Float32(*value)),
                Some(CborEvent::Float64(value)) => Some(CborValue::Float64(*value)),
                _ => None,
            };
            if let Some(value) = float
                && !preferred_float_wire(&value, &input[start..cursor.pos], options.limits)
            {
                return Err(cursor.error(CborErrorKind::NonMinimalEncoding, start));
            }
        }
        if complete {
            scan_complete(&mut stack, &mut root_done);
        }
        if events.len() + 1 > options.limits.max_events {
            return Err(cursor.error(CborErrorKind::LimitExceeded, cursor.pos));
        }
    }
    if cursor.pos != input.len() {
        return Err(cursor.error(CborErrorKind::TrailingData, cursor.pos));
    }
    events.push(CborEvent::StreamEnd);
    Ok(events)
}

fn scan_events(input: &[u8], options: CborDecodeOptions) -> Result<Vec<CborEvent>, CborError> {
    scan_events_inner(input, options, true)
}

#[derive(Debug)]
pub struct CborReader {
    events: Vec<CborEvent>,
    index: usize,
    eof_returned: bool,
    finished: bool,
}

impl CborReader {
    pub fn from_bytes(input: &[u8], options: CborDecodeOptions) -> Result<Self, CborError> {
        Ok(Self {
            events: scan_events(input, options)?,
            index: 0,
            eof_returned: false,
            finished: false,
        })
    }

    pub fn from_reader<R: std::io::Read>(
        mut source: R,
        options: CborDecodeOptions,
    ) -> Result<Self, CborError> {
        let mut input = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let n = source
                .read(&mut chunk)
                .map_err(|_| CborError::at(CborErrorKind::IoError, input.len(), input.len()))?;
            if n == 0 {
                break;
            }
            if input
                .len()
                .checked_add(n)
                .is_none_or(|size| size > options.limits.max_document_bytes)
            {
                return Err(CborError::at(
                    CborErrorKind::LimitExceeded,
                    input.len(),
                    input.len(),
                ));
            }
            input.extend_from_slice(&chunk[..n]);
        }
        Self::from_bytes(&input, options)
    }

    #[expect(
        clippy::should_implement_trait,
        reason = "CborReader.next is the source-language fallible event API"
    )]
    pub fn next(&mut self) -> Result<Option<CborEvent>, CborError> {
        if self.finished {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        if self.eof_returned {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        let event = self.events.get(self.index).cloned();
        if event.is_none() {
            self.eof_returned = true;
        }
        self.index += usize::from(event.is_some());
        Ok(event)
    }

    pub fn own(&self, event: &CborEvent) -> Result<CborEvent, CborError> {
        if self.finished {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        Ok(event.clone())
    }

    pub fn finish(&mut self) -> Result<(), CborError> {
        if self.finished {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        self.finished = true;
        if self.index < self.events.len() {
            return Err(CborError::at(CborErrorKind::TrailingData, 0, 0));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
enum WriteFrame {
    Array {
        expected: Option<usize>,
        count: usize,
    },
    Map {
        expected: Option<usize>,
        count: usize,
        stage: u8,
        key_start: usize,
        previous_key: Option<Vec<u8>>,
    },
    Tag,
    Bytes {
        expected: Option<usize>,
        count: usize,
        chunks: usize,
    },
    Text {
        expected: Option<usize>,
        count: usize,
        chunks: usize,
    },
}

#[derive(Debug, Clone, Default)]
struct WriteState {
    out: Vec<u8>,
    frames: Vec<WriteFrame>,
    started: bool,
    root_done: bool,
    ended: bool,
    events: usize,
    tags: usize,
    simple_values: usize,
}

impl WriteState {
    fn error(&self, kind: CborErrorKind) -> CborError {
        CborError::at(kind, self.out.len(), self.out.len())
    }

    fn before_item(&self, limits: CborLimits) -> Result<(), CborError> {
        if !self.started || self.ended || self.root_done {
            return Err(self.error(CborErrorKind::InvalidLength));
        }
        match self.frames.last() {
            Some(WriteFrame::Array {
                expected: Some(n),
                count,
            }) if count >= n => Err(self.error(CborErrorKind::InvalidLength)),
            Some(WriteFrame::Array { count, .. }) if *count >= limits.max_array_items => {
                Err(self.error(CborErrorKind::LimitExceeded))
            }
            Some(WriteFrame::Map { stage, .. }) if *stage != 1 && *stage != 3 => {
                Err(self.error(CborErrorKind::TypeMismatch))
            }
            Some(WriteFrame::Bytes { .. } | WriteFrame::Text { .. }) => {
                Err(self.error(CborErrorKind::InvalidChunk))
            }
            _ => Ok(()),
        }
    }

    fn complete(&mut self, deterministic: bool) -> Result<(), CborError> {
        loop {
            match self.frames.last_mut() {
                Some(WriteFrame::Array { count, .. }) => {
                    *count += 1;
                    return Ok(());
                }
                Some(WriteFrame::Map {
                    stage,
                    key_start,
                    previous_key,
                    ..
                }) if *stage == 1 => {
                    let key = self.out[*key_start..].to_vec();
                    if deterministic && let Some(previous) = previous_key {
                        if previous == &key {
                            return Err(self.error(CborErrorKind::DeterministicKeyCollision));
                        }
                        if previous.as_slice() > key.as_slice() {
                            return Err(self.error(CborErrorKind::OutOfOrderKey));
                        }
                    }
                    *previous_key = Some(key);
                    *stage = 2;
                    return Ok(());
                }
                Some(WriteFrame::Map { stage, count, .. }) if *stage == 3 => {
                    *stage = 0;
                    *count += 1;
                    return Ok(());
                }
                Some(WriteFrame::Tag) => {
                    self.frames.pop();
                }
                None => {
                    self.root_done = true;
                    return Ok(());
                }
                _ => return Err(self.error(CborErrorKind::TypeMismatch)),
            }
        }
    }

    fn write_chunk(
        &mut self,
        raw: &[u8],
        is_bytes: bool,
        options: CborEncodeOptions,
    ) -> Result<(), CborError> {
        let offset = self.out.len();
        let frame = self
            .frames
            .last_mut()
            .ok_or_else(|| CborError::at(CborErrorKind::InvalidChunk, offset, offset))?;
        let (expected, count, chunks) = match frame {
            WriteFrame::Bytes {
                expected,
                count,
                chunks,
            } if is_bytes => (expected, count, chunks),
            WriteFrame::Text {
                expected,
                count,
                chunks,
            } if !is_bytes => (expected, count, chunks),
            _ => return Err(CborError::at(CborErrorKind::InvalidChunk, offset, offset)),
        };
        *chunks += 1;
        if *chunks > options.limits.max_chunks {
            return Err(CborError::at(CborErrorKind::TooManyChunks, offset, offset));
        }
        *count = count
            .checked_add(raw.len())
            .ok_or_else(|| CborError::at(CborErrorKind::LimitExceeded, offset, offset))?;
        let limit = if is_bytes {
            options.limits.max_byte_string_bytes
        } else {
            options.limits.max_string_bytes
        };
        if *count > limit || expected.is_some_and(|n| *count > n) {
            return Err(CborError::at(CborErrorKind::LimitExceeded, offset, offset));
        }
        if expected.is_none() {
            push_head(
                &mut self.out,
                if is_bytes { 2 } else { 3 },
                raw.len() as u64,
                options.limits,
            )?;
        }
        push_bytes(&mut self.out, raw, options.limits)
    }

    fn write(&mut self, event: CborEvent, options: CborEncodeOptions) -> Result<(), CborError> {
        self.events += 1;
        if self.events > options.limits.max_events {
            return Err(self.error(CborErrorKind::LimitExceeded));
        }
        match event {
            CborEvent::StreamStart if !self.started => {
                self.started = true;
                return Ok(());
            }
            CborEvent::StreamEnd if self.root_done && self.frames.is_empty() && !self.ended => {
                self.ended = true;
                return Ok(());
            }
            CborEvent::MapKey => {
                if let Some(WriteFrame::Map {
                    stage,
                    key_start,
                    expected,
                    count,
                    ..
                }) = self.frames.last_mut()
                    && *stage == 0
                    && expected.is_none_or(|n| *count < n)
                    && *count < options.limits.max_map_pairs
                {
                    *stage = 1;
                    *key_start = self.out.len();
                    return Ok(());
                }
                return Err(self.error(CborErrorKind::TypeMismatch));
            }
            CborEvent::EndArray => {
                let Some(WriteFrame::Array { expected, count }) = self.frames.last() else {
                    return Err(self.error(CborErrorKind::TypeMismatch));
                };
                if expected.is_some_and(|n| n != *count) {
                    return Err(self.error(CborErrorKind::InvalidLength));
                }
                if expected.is_none() {
                    push_bytes(&mut self.out, &[0xff], options.limits)?;
                }
                self.frames.pop();
                return self.complete(options.deterministic);
            }
            CborEvent::EndMap => {
                let Some(WriteFrame::Map {
                    expected,
                    count,
                    stage,
                    ..
                }) = self.frames.last()
                else {
                    return Err(self.error(CborErrorKind::TypeMismatch));
                };
                if *stage != 0 || expected.is_some_and(|n| n != *count) {
                    return Err(self.error(CborErrorKind::InvalidLength));
                }
                if expected.is_none() {
                    push_bytes(&mut self.out, &[0xff], options.limits)?;
                }
                self.frames.pop();
                return self.complete(options.deterministic);
            }
            CborEvent::EndBytes | CborEvent::EndText => {
                let frame = self
                    .frames
                    .last()
                    .ok_or_else(|| self.error(CborErrorKind::InvalidChunk))?;
                let (expected, count, is_bytes) = match frame {
                    WriteFrame::Bytes {
                        expected, count, ..
                    } => (*expected, *count, true),
                    WriteFrame::Text {
                        expected, count, ..
                    } => (*expected, *count, false),
                    _ => return Err(self.error(CborErrorKind::InvalidChunk)),
                };
                if is_bytes != matches!(event, CborEvent::EndBytes)
                    || expected.is_some_and(|n| n != count)
                {
                    return Err(self.error(CborErrorKind::InvalidChunk));
                }
                if expected.is_none() {
                    push_bytes(&mut self.out, &[0xff], options.limits)?;
                }
                self.frames.pop();
                return self.complete(options.deterministic);
            }
            CborEvent::ByteChunk(ref bytes) => return self.write_chunk(bytes, true, options),
            CborEvent::TextChunk(ref text) => {
                return self.write_chunk(text.as_bytes(), false, options);
            }
            _ => {}
        }
        if let Some(WriteFrame::Map { stage, .. }) = self.frames.last_mut()
            && *stage == 2
        {
            *stage = 3;
        }
        self.before_item(options.limits)?;
        match event {
            CborEvent::Null => push_bytes(&mut self.out, &[0xf6], options.limits)?,
            CborEvent::Undefined => push_bytes(&mut self.out, &[0xf7], options.limits)?,
            CborEvent::Bool(b) => push_bytes(
                &mut self.out,
                &[if b { 0xf5 } else { 0xf4 }],
                options.limits,
            )?,
            CborEvent::Simple(n) => {
                if (20..=31).contains(&n) {
                    return Err(self.error(CborErrorKind::InvalidSimpleValue));
                }
                self.simple_values += 1;
                if self.simple_values > options.limits.max_simple_values {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                if n < 20 {
                    push_bytes(&mut self.out, &[0xe0 | n], options.limits)?;
                } else {
                    push_bytes(&mut self.out, &[0xf8, n], options.limits)?;
                }
            }
            CborEvent::UInt(n) => push_head(&mut self.out, 0, n, options.limits)?,
            CborEvent::Negative(n) => push_head(&mut self.out, 1, n, options.limits)?,
            CborEvent::Float16(n) => push_float(
                &mut self.out,
                &CborValue::Float16(n),
                options.deterministic,
                options.limits,
            )?,
            CborEvent::Float32(n) => push_float(
                &mut self.out,
                &CborValue::Float32(n),
                options.deterministic,
                options.limits,
            )?,
            CborEvent::Float64(n) => push_float(
                &mut self.out,
                &CborValue::Float64(n),
                options.deterministic,
                options.limits,
            )?,
            CborEvent::Bytes(bytes) => {
                if bytes.len() > options.limits.max_byte_string_bytes {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                push_head(&mut self.out, 2, bytes.len() as u64, options.limits)?;
                push_bytes(&mut self.out, &bytes, options.limits)?;
            }
            CborEvent::Text(text) => {
                if text.len() > options.limits.max_string_bytes {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                push_head(&mut self.out, 3, text.len() as u64, options.limits)?;
                push_bytes(&mut self.out, text.as_bytes(), options.limits)?;
            }
            CborEvent::StartBytes(expected) | CborEvent::StartText(expected) => {
                let is_bytes = matches!(event, CborEvent::StartBytes(_));
                if options.deterministic && expected.is_none() {
                    return Err(self.error(CborErrorKind::IndefiniteNotAllowed));
                }
                let limit = if is_bytes {
                    options.limits.max_byte_string_bytes
                } else {
                    options.limits.max_string_bytes
                };
                if expected.is_some_and(|n| n > limit) {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                if let Some(n) = expected {
                    push_head(
                        &mut self.out,
                        if is_bytes { 2 } else { 3 },
                        n as u64,
                        options.limits,
                    )?;
                } else {
                    push_bytes(
                        &mut self.out,
                        &[if is_bytes { 0x5f } else { 0x7f }],
                        options.limits,
                    )?;
                }
                self.frames.push(if is_bytes {
                    WriteFrame::Bytes {
                        expected,
                        count: 0,
                        chunks: 0,
                    }
                } else {
                    WriteFrame::Text {
                        expected,
                        count: 0,
                        chunks: 0,
                    }
                });
                if self.frames.len() > options.limits.max_depth {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                return Ok(());
            }
            CborEvent::StartArray(expected) | CborEvent::StartMap(expected) => {
                let is_array = matches!(event, CborEvent::StartArray(_));
                if options.deterministic && expected.is_none() {
                    return Err(self.error(CborErrorKind::IndefiniteNotAllowed));
                }
                let limit = if is_array {
                    options.limits.max_array_items
                } else {
                    options.limits.max_map_pairs
                };
                if expected.is_some_and(|n| n > limit) {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                if let Some(n) = expected {
                    push_head(
                        &mut self.out,
                        if is_array { 4 } else { 5 },
                        n as u64,
                        options.limits,
                    )?;
                } else {
                    push_bytes(
                        &mut self.out,
                        &[if is_array { 0x9f } else { 0xbf }],
                        options.limits,
                    )?;
                }
                self.frames.push(if is_array {
                    WriteFrame::Array { expected, count: 0 }
                } else {
                    WriteFrame::Map {
                        expected,
                        count: 0,
                        stage: 0,
                        key_start: 0,
                        previous_key: None,
                    }
                });
                if self.frames.len() > options.limits.max_depth {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                return Ok(());
            }
            CborEvent::Tag(n) => {
                self.tags += 1;
                if self.tags > options.limits.max_tags {
                    return Err(self.error(CborErrorKind::TooManyTags));
                }
                push_head(&mut self.out, 6, n, options.limits)?;
                self.frames.push(WriteFrame::Tag);
                if self.frames.len() > options.limits.max_depth {
                    return Err(self.error(CborErrorKind::LimitExceeded));
                }
                return Ok(());
            }
            _ => return Err(self.error(CborErrorKind::TypeMismatch)),
        }
        self.complete(options.deterministic)
    }
}

pub struct CborWriter<W: std::io::Write> {
    sink: Option<W>,
    state: WriteState,
    options: CborEncodeOptions,
    terminal: bool,
}

impl<W: std::io::Write> CborWriter<W> {
    fn number_range(&mut self) -> CborError {
        self.terminal = true;
        self.state.error(CborErrorKind::NumberRange)
    }

    pub fn to_writer(sink: W, options: CborEncodeOptions) -> Result<Self, CborError> {
        CborLimits::create(options.limits)?;
        Ok(Self {
            sink: Some(sink),
            state: WriteState::default(),
            options,
            terminal: false,
        })
    }

    pub fn write(&mut self, event: CborEvent) -> Result<(), CborError> {
        if self.terminal {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        match self.state.write(event, self.options) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.terminal = true;
                Err(error)
            }
        }
    }

    pub fn finish(&mut self) -> Result<W, CborError> {
        if self.terminal {
            return Err(CborError::at(CborErrorKind::Closed, 0, 0));
        }
        self.terminal = true;
        if !self.state.ended {
            return Err(self.state.error(CborErrorKind::UnexpectedEof));
        }
        let mut sink = self
            .sink
            .take()
            .ok_or_else(|| self.state.error(CborErrorKind::Closed))?;
        sink.write_all(&self.state.out).map_err(|error| {
            self.state
                .error(if error.kind() == std::io::ErrorKind::WriteZero {
                    CborErrorKind::NoProgress
                } else {
                    CborErrorKind::IoError
                })
        })?;
        sink.flush()
            .map_err(|_| self.state.error(CborErrorKind::IoError))?;
        Ok(sink)
    }
}

impl<W: std::io::Write> Encoder<Cbor, CborError> for CborWriter<W> {
    fn write_event(&mut self, event: Event) -> Result<(), CborError> {
        if self.terminal {
            return Err(self.state.error(CborErrorKind::Closed));
        }
        let cbor = match event {
            Event::Null => CborEvent::Null,
            Event::Bool(value) => CborEvent::Bool(value),
            Event::Int(value) if value >= 0 => {
                CborEvent::UInt(u64::try_from(value).map_err(|_| self.number_range())?)
            }
            Event::Int(value) => {
                let magnitude = value
                    .checked_neg()
                    .and_then(|n| n.checked_sub(1))
                    .ok_or_else(|| self.number_range())?;
                CborEvent::Negative(u64::try_from(magnitude).map_err(|_| self.number_range())?)
            }
            Event::UInt(value) => {
                CborEvent::UInt(u64::try_from(value).map_err(|_| self.number_range())?)
            }
            Event::Float(value) => CborEvent::Float64(value.to_bits()),
            Event::Float32(value) => CborEvent::Float32(value),
            Event::Float64(value) => CborEvent::Float64(value),
            Event::String(value) => CborEvent::Text(value),
            Event::Bytes(value) => CborEvent::Bytes(value),
            Event::StartArray(length) => CborEvent::StartArray(length),
            Event::EndArray => CborEvent::EndArray,
            Event::StartMap(length) => CborEvent::StartMap(length),
            Event::MapKey => CborEvent::MapKey,
            Event::EndMap => CborEvent::EndMap,
            Event::StartRecord { fields, .. } => CborEvent::StartMap(fields),
            Event::Field(name) => {
                self.write(CborEvent::MapKey)?;
                CborEvent::Text(name)
            }
            Event::EndRecord => CborEvent::EndMap,
            Event::StartEnum { variant, .. } => {
                self.write(CborEvent::StartMap(Some(1)))?;
                self.write(CborEvent::MapKey)?;
                CborEvent::Text(variant)
            }
            Event::EndEnum => {
                if matches!(
                    self.state.frames.last(),
                    Some(WriteFrame::Map { stage: 2, .. })
                ) {
                    self.write(CborEvent::Null)?;
                }
                CborEvent::EndMap
            }
        };
        self.write(cbor)
    }
}

/// Encode a statically dispatched value without constructing a dynamic CBOR tree.
pub fn encode_typed<T: Encode<Cbor>>(
    value: &T,
    options: CborEncodeOptions,
) -> Result<Vec<u8>, CborError> {
    let mut writer = CborWriter::to_writer(Vec::new(), options)?;
    writer.write(CborEvent::StreamStart)?;
    value.encode(&mut writer)?;
    writer.write(CborEvent::StreamEnd)?;
    writer.finish()
}

fn common_events(input: &[u8], options: CborDecodeOptions) -> Result<Vec<Event>, CborError> {
    let source = scan_events_inner(input, options, false)?;
    let mut events = Vec::with_capacity(source.len());
    let mut index = 0usize;
    while index < source.len() {
        let event = match &source[index] {
            CborEvent::StreamStart | CborEvent::StreamEnd => {
                index += 1;
                continue;
            }
            CborEvent::Null => Event::Null,
            CborEvent::Undefined | CborEvent::Simple(_) | CborEvent::Tag(_) => {
                return Err(CborError::at(CborErrorKind::TypeMismatch, 0, 0));
            }
            CborEvent::Bool(value) => Event::Bool(*value),
            CborEvent::UInt(value) => Event::UInt(u128::from(*value)),
            CborEvent::Negative(value) => Event::Int(-1 - i128::from(*value)),
            CborEvent::Float16(value) => Event::Float32(half_to_f32(value.bits).to_bits()),
            CborEvent::Float32(value) => Event::Float32(*value),
            CborEvent::Float64(value) => Event::Float64(*value),
            CborEvent::Bytes(value) => Event::Bytes(value.clone()),
            CborEvent::Text(value) => Event::String(value.clone()),
            CborEvent::StartArray(value) => Event::StartArray(*value),
            CborEvent::EndArray => Event::EndArray,
            CborEvent::StartMap(value) => Event::StartMap(*value),
            CborEvent::MapKey => Event::MapKey,
            CborEvent::EndMap => Event::EndMap,
            CborEvent::StartBytes(_) | CborEvent::StartText(_) => {
                let is_bytes = matches!(&source[index], CborEvent::StartBytes(_));
                let mut bytes = Vec::new();
                index += 1;
                while index < source.len() {
                    match &source[index] {
                        CborEvent::ByteChunk(chunk) if is_bytes => bytes.extend_from_slice(chunk),
                        CborEvent::TextChunk(chunk) if !is_bytes => {
                            bytes.extend_from_slice(chunk.as_bytes())
                        }
                        CborEvent::EndBytes if is_bytes => break,
                        CborEvent::EndText if !is_bytes => break,
                        _ => return Err(CborError::at(CborErrorKind::TypeMismatch, 0, 0)),
                    }
                    index += 1;
                }
                if is_bytes {
                    Event::Bytes(bytes)
                } else {
                    Event::String(
                        String::from_utf8(bytes)
                            .map_err(|_| CborError::at(CborErrorKind::InvalidUtf8, 0, 0))?,
                    )
                }
            }
            CborEvent::ByteChunk(_)
            | CborEvent::TextChunk(_)
            | CborEvent::EndBytes
            | CborEvent::EndText => return Err(CborError::at(CborErrorKind::TypeMismatch, 0, 0)),
        };
        events.push(event);
        index += 1;
    }
    Ok(events)
}

struct CborTypedDecoder {
    events: Vec<Event>,
    index: usize,
    limits: serialization::Limits,
    duplicates: serialization::MapDuplicatePolicy,
}

impl Decoder<Cbor, CborError> for CborTypedDecoder {
    fn limits(&self) -> serialization::Limits {
        self.limits
    }

    fn map_duplicate_policy(&self) -> serialization::MapDuplicatePolicy {
        self.duplicates
    }

    fn peek_event(&mut self) -> Result<Option<Event>, CborError> {
        Ok(self.events.get(self.index).cloned())
    }

    fn next(&mut self) -> Result<Option<Event>, CborError> {
        let event = self.events.get(self.index).cloned();
        self.index += usize::from(event.is_some());
        Ok(event)
    }

    fn reject(&mut self, error: SerializationError) -> CborError {
        error.into()
    }
}

pub fn decode_typed<T: Decode<Cbor>>(
    input: &[u8],
    options: CborDecodeOptions,
) -> Result<T, CborError> {
    let events = common_events(input, options)?;
    let duplicates = match options.typed_map_duplicates {
        CborDuplicatePolicy::Reject => serialization::MapDuplicatePolicy::Reject,
        CborDuplicatePolicy::First => serialization::MapDuplicatePolicy::First,
        CborDuplicatePolicy::Last => serialization::MapDuplicatePolicy::Last,
        CborDuplicatePolicy::Preserve => {
            return Err(CborError::at(CborErrorKind::TypeMismatch, 0, 0));
        }
    };
    let mut decoder = CborTypedDecoder {
        events,
        index: 0,
        duplicates,
        limits: serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_events,
            max_bytes: options.limits.max_document_bytes,
            max_container_items: options
                .limits
                .max_array_items
                .min(options.limits.max_map_pairs),
        },
    };
    let value = T::decode(&mut decoder)?;
    if decoder.index != decoder.events.len() {
        return Err(CborError::at(CborErrorKind::TrailingData, 0, input.len()));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_types_preserve_widths_tags_and_map_order() {
        let input = [
            0x86, 0x18, 0x18, 0x38, 0x00, 0xf7, 0xf9, 0x7e, 0x01, 0xc1, 0x01, 0xa2, 0x01, 0x02,
            0x01, 0x03,
        ];
        let value = parse(&input, CborDecodeOptions::default()).unwrap();
        let CborValue::Array(items) = &value else {
            panic!("expected array")
        };
        assert_eq!(items[0], CborValue::UInt(24));
        assert_eq!(items[1], CborValue::Negative(0));
        assert_eq!(items[2], CborValue::Undefined);
        assert_eq!(items[3], CborValue::Float16(CborFloat16 { bits: 0x7e01 }));
        assert_eq!(
            items[4],
            CborValue::Tag(CborTag {
                number: 1,
                value: Box::new(CborValue::UInt(1))
            })
        );
        assert!(matches!(&items[5], CborValue::Map(pairs) if pairs.len() == 2));
        assert_eq!(
            encode(&value, CborEncodeOptions::default()).unwrap(),
            [
                0x86, 0x18, 0x18, 0x20, 0xf7, 0xf9, 0x7e, 0x01, 0xc1, 0x01, 0xa2, 0x01, 0x02, 0x01,
                0x03
            ]
        );
        assert_eq!(
            raw(&input, CborDecodeOptions::default())
                .unwrap()
                .as_bytes(),
            input
        );
    }

    #[test]
    fn indefinite_forms_and_chunk_utf8_are_checked() {
        let input = [
            0x9f, 0x5f, 0x42, 0x01, 0x02, 0x40, 0xff, 0x7f, 0x62, b'h', b'i', 0x60, 0xff, 0xbf,
            0x01, 0xf5, 0xff, 0xff,
        ];
        let value = parse(&input, CborDecodeOptions::default()).unwrap();
        assert!(matches!(value, CborValue::Array(ref items) if items.len() == 3));
        let events = scan_events(&input, CborDecodeOptions::default()).unwrap();
        assert!(events.contains(&CborEvent::StartBytes(None)));
        assert!(events.contains(&CborEvent::ByteChunk(Vec::new())));
        assert!(events.contains(&CborEvent::StartMap(None)));
        assert_eq!(
            parse(&[0x7f, 0x41, 0xff, 0xff], CborDecodeOptions::default())
                .unwrap_err()
                .kind,
            CborErrorKind::InvalidChunk
        );
        assert_eq!(
            parse(
                &[0x7f, 0x62, 0xc3, 0x00, 0xff],
                CborDecodeOptions::default()
            )
            .unwrap_err()
            .kind,
            CborErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn deterministic_encoding_sorts_keys_and_shortens_floats() {
        let value = CborValue::Map(vec![
            CborEntry {
                key: CborValue::Text("b".into()),
                value: CborValue::Float64((-0.0f64).to_bits()),
            },
            CborEntry {
                key: CborValue::UInt(1),
                value: CborValue::Float32(1.5f32.to_bits()),
            },
        ]);
        assert_eq!(
            encode_deterministic(&value, CborLimits::default()).unwrap(),
            [0xa2, 0x01, 0xf9, 0x3e, 0x00, 0x61, b'b', 0xf9, 0x80, 0x00]
        );
        assert_eq!(
            encode_deterministic(
                &CborValue::Float64(f64::NAN.to_bits()),
                CborLimits::default()
            )
            .unwrap(),
            [0xf9, 0x7e, 0x00]
        );
        let duplicate = CborValue::Map(vec![
            CborEntry {
                key: CborValue::Float32(f32::NAN.to_bits()),
                value: CborValue::Null,
            },
            CborEntry {
                key: CborValue::Float64(f64::NAN.to_bits()),
                value: CborValue::Null,
            },
        ]);
        assert_eq!(
            encode_deterministic(&duplicate, CborLimits::default())
                .unwrap_err()
                .kind,
            CborErrorKind::DeterministicKeyCollision
        );
    }

    #[test]
    fn malformed_and_limits_return_no_value() {
        let options = CborDecodeOptions::default();
        for (bytes, kind) in [
            (&[0xff][..], CborErrorKind::InvalidBreak),
            (&[0x18][..], CborErrorKind::UnexpectedEof),
            (&[0x00, 0x01][..], CborErrorKind::TrailingData),
            (&[0x81, 0xff][..], CborErrorKind::InvalidBreak),
            (&[0xf8, 0x14][..], CborErrorKind::InvalidSimpleValue),
            (&[0x1c][..], CborErrorKind::InvalidAdditionalInfo),
        ] {
            assert_eq!(parse(bytes, options).unwrap_err().kind, kind);
        }
        let mut strict = options;
        strict.non_minimal = CborNonMinimalPolicy::Reject;
        assert_eq!(
            parse(&[0x18, 0x01], strict).unwrap_err().kind,
            CborErrorKind::NonMinimalEncoding
        );
        assert_eq!(
            parse(&[0xfa, 0x3f, 0xc0, 0x00, 0x00], strict)
                .unwrap_err()
                .kind,
            CborErrorKind::NonMinimalEncoding
        );
        assert_eq!(
            scan_events_inner(&[0xfa, 0x3f, 0xc0, 0x00, 0x00], strict, false)
                .unwrap_err()
                .kind,
            CborErrorKind::NonMinimalEncoding
        );
        let mut bounded = options;
        bounded.limits.max_array_items = 1;
        assert_eq!(
            parse(&[0x82, 0x00, 0x00], bounded).unwrap_err().kind,
            CborErrorKind::LimitExceeded
        );
        let mut bounded = options;
        bounded.limits.max_map_pairs = 1;
        bounded.dynamic_map_duplicates = CborDuplicatePolicy::First;
        assert_eq!(
            parse(&[0xbf, 0x00, 0x01, 0x00, 0x02, 0xff], bounded)
                .unwrap_err()
                .kind,
            CborErrorKind::LimitExceeded
        );
        let nested = parse(&[0x81, 0xbf, 0x01, 0xff], options).unwrap_err();
        assert_eq!(nested.kind, CborErrorKind::InvalidBreak);
        assert_eq!(
            nested.path,
            [
                CborPath::ArrayIndex(0),
                CborPath::MapEntry(0),
                CborPath::MapValue
            ]
        );
    }

    #[test]
    fn duplicate_policies_and_raw_preserve_first_position_and_wire() {
        let input = [0xa3, 0x01, 0x02, 0x03, 0x04, 0x01, 0x05];
        let mut options = CborDecodeOptions {
            dynamic_map_duplicates: CborDuplicatePolicy::Reject,
            ..CborDecodeOptions::default()
        };
        assert_eq!(
            parse(&input, options).unwrap_err().kind,
            CborErrorKind::DuplicateKey
        );
        options.dynamic_map_duplicates = CborDuplicatePolicy::Last;
        let CborValue::Map(entries) = parse(&input, options).unwrap() else {
            panic!("expected map")
        };
        assert_eq!(
            entries[0],
            CborEntry {
                key: CborValue::UInt(1),
                value: CborValue::UInt(5)
            }
        );
        assert_eq!(entries[1].key, CborValue::UInt(3));
        assert_eq!(raw(&input, options).unwrap().as_bytes(), input);
    }

    #[test]
    fn reader_is_invariant_to_one_byte_input_and_has_terminal_finish() {
        struct OneByte<'a>(&'a [u8]);
        impl std::io::Read for OneByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Ok(0);
                }
                buf[0] = self.0[0];
                self.0 = &self.0[1..];
                Ok(1)
            }
        }
        let input = [0x9f, 0x01, 0x02, 0xff];
        let mut direct = CborReader::from_bytes(&input, CborDecodeOptions::default()).unwrap();
        let mut chunked =
            CborReader::from_reader(OneByte(&input), CborDecodeOptions::default()).unwrap();
        loop {
            let left = direct.next().unwrap();
            let right = chunked.next().unwrap();
            assert_eq!(left, right);
            if left.is_none() {
                break;
            }
        }
        direct.finish().unwrap();
        assert_eq!(direct.next().unwrap_err().kind, CborErrorKind::Closed);
    }

    #[test]
    fn writer_round_trips_indefinite_frames_and_closes_on_error() {
        let input = [
            0x9f, 0x5f, 0x42, 0x01, 0x02, 0x40, 0xff, 0x7f, 0x62, b'h', b'i', 0x60, 0xff, 0xbf,
            0x01, 0xf5, 0xff, 0xff,
        ];
        let mut reader = CborReader::from_bytes(&input, CborDecodeOptions::default()).unwrap();
        let mut writer = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
        while let Some(event) = reader.next().unwrap() {
            writer.write(event).unwrap();
        }
        assert_eq!(writer.finish().unwrap(), input);

        let mut invalid = CborWriter::to_writer(Vec::new(), CborEncodeOptions::default()).unwrap();
        invalid.write(CborEvent::StreamStart).unwrap();
        invalid.write(CborEvent::StartMap(Some(1))).unwrap();
        assert_eq!(
            invalid.write(CborEvent::UInt(1)).unwrap_err().kind,
            CborErrorKind::TypeMismatch
        );
        assert_eq!(
            invalid.write(CborEvent::MapKey).unwrap_err().kind,
            CborErrorKind::Closed
        );
    }

    #[test]
    fn deterministic_writer_checks_order_and_definite_lengths() {
        let options = CborEncodeOptions {
            deterministic: true,
            ..CborEncodeOptions::default()
        };
        let mut writer = CborWriter::to_writer(Vec::new(), options).unwrap();
        writer.write(CborEvent::StreamStart).unwrap();
        writer.write(CborEvent::StartMap(Some(2))).unwrap();
        writer.write(CborEvent::MapKey).unwrap();
        writer.write(CborEvent::Text("b".into())).unwrap();
        writer.write(CborEvent::UInt(1)).unwrap();
        writer.write(CborEvent::MapKey).unwrap();
        assert_eq!(
            writer.write(CborEvent::UInt(0)).unwrap_err().kind,
            CborErrorKind::OutOfOrderKey
        );
        assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);

        let mut indefinite = CborWriter::to_writer(Vec::new(), options).unwrap();
        indefinite.write(CborEvent::StreamStart).unwrap();
        assert_eq!(
            indefinite
                .write(CborEvent::StartArray(None))
                .unwrap_err()
                .kind,
            CborErrorKind::IndefiniteNotAllowed
        );
    }

    #[test]
    fn exact_half_round_trip_covers_every_finite_wire_pattern() {
        for bits in 0..=u16::MAX {
            if bits & 0x7c00 == 0x7c00 && bits & 0x03ff != 0 {
                continue;
            }
            assert_eq!(
                exact_half(half_to_f32(bits)),
                Some(bits),
                "half bits {bits:04x}"
            );
        }
    }

    #[test]
    fn typed_collections_use_the_common_static_protocol() {
        let values = vec![1u64, 24, u64::MAX];
        let bytes = encode_typed(&values, CborEncodeOptions::default()).unwrap();
        assert_eq!(
            decode_typed::<Vec<u64>>(&bytes, CborDecodeOptions::default()).unwrap(),
            values
        );
        let indefinite = [0x9f, 0x01, 0x18, 0x18, 0xff];
        assert_eq!(
            decode_typed::<Vec<u64>>(&indefinite, CborDecodeOptions::default()).unwrap(),
            vec![1, 24]
        );
        let mut map = std::collections::BTreeMap::new();
        map.insert("alpha".to_string(), true);
        map.insert("beta".to_string(), false);
        let bytes = encode_typed(&map, CborEncodeOptions::default()).unwrap();
        assert_eq!(
            decode_typed::<std::collections::BTreeMap<String, bool>>(
                &bytes,
                CborDecodeOptions::default()
            )
            .unwrap(),
            map
        );
        assert_eq!(
            decode_typed::<u64>(&[0x61, b'x'], CborDecodeOptions::default())
                .unwrap_err()
                .kind,
            CborErrorKind::TypeMismatch
        );
        let duplicate = [0xa2, 0x01, 0x02, 0x01, 0x03];
        assert_eq!(
            decode_typed::<std::collections::BTreeMap<u64, u64>>(
                &duplicate,
                CborDecodeOptions::default()
            )
            .unwrap_err()
            .kind,
            CborErrorKind::DuplicateKey
        );
        let mut options = CborDecodeOptions {
            typed_map_duplicates: CborDuplicatePolicy::First,
            ..CborDecodeOptions::default()
        };
        assert_eq!(
            decode_typed::<std::collections::BTreeMap<u64, u64>>(&duplicate, options)
                .unwrap()
                .get(&1),
            Some(&2)
        );
        options.typed_map_duplicates = CborDuplicatePolicy::Last;
        assert_eq!(
            decode_typed::<std::collections::BTreeMap<u64, u64>>(&duplicate, options)
                .unwrap()
                .get(&1),
            Some(&3)
        );
    }

    #[test]
    fn direct_event_scan_agrees_with_value_parser_on_bounded_inputs() {
        let options = CborDecodeOptions::default();
        let mut seed = 0xC80Au64;
        for case in 0..4096 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (seed >> 32) as usize % 48;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                bytes.push((seed >> 32) as u8);
            }
            assert_eq!(
                parse(&bytes, options).is_ok(),
                scan_events_inner(&bytes, options, false).is_ok(),
                "case {case}: {bytes:02x?}"
            );
        }
        assert_eq!(parse(&[0xf8, 0x00], options).unwrap(), CborValue::Simple(0));
        let mut strict = options;
        strict.non_minimal = CborNonMinimalPolicy::Reject;
        assert_eq!(
            scan_events_inner(&[0xf8, 0x00], strict, false)
                .unwrap_err()
                .kind,
            CborErrorKind::NonMinimalEncoding
        );
    }

    #[test]
    fn depth_and_output_limits_reject_before_publication() {
        let mut options = CborDecodeOptions::default();
        options.limits.max_depth = 2;
        assert_eq!(
            parse(&[0x81, 0x81, 0x81, 0x00], options).unwrap_err().kind,
            CborErrorKind::LimitExceeded
        );
        let nested = CborValue::Array(vec![CborValue::Array(vec![CborValue::Array(vec![
            CborValue::UInt(0),
        ])])]);
        let limits = CborLimits {
            max_depth: 2,
            ..CborLimits::default()
        };
        assert_eq!(
            encode_deterministic(&nested, limits).unwrap_err().kind,
            CborErrorKind::LimitExceeded
        );

        let mut sink = Vec::new();
        let writer_options = CborEncodeOptions {
            limits: CborLimits {
                max_output_bytes: 1,
                ..CborLimits::default()
            },
            deterministic: false,
        };
        let mut writer = CborWriter::to_writer(&mut sink, writer_options).unwrap();
        writer.write(CborEvent::StreamStart).unwrap();
        assert_eq!(
            writer.write(CborEvent::Text("x".into())).unwrap_err().kind,
            CborErrorKind::LimitExceeded
        );
        assert_eq!(writer.finish().unwrap_err().kind, CborErrorKind::Closed);
        drop(writer);
        assert!(sink.is_empty());
    }

    #[test]
    fn reader_io_failure_does_not_publish_events() {
        struct Fails;
        impl std::io::Read for Fails {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("deliberate read failure"))
            }
        }
        assert_eq!(
            CborReader::from_reader(Fails, CborDecodeOptions::default())
                .unwrap_err()
                .kind,
            CborErrorKind::IoError
        );
    }

    #[test]
    fn writer_reports_no_progress_and_stays_terminal() {
        struct NoProgress;
        impl std::io::Write for NoProgress {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Ok(0)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = CborWriter::to_writer(NoProgress, CborEncodeOptions::default()).unwrap();
        writer.write(CborEvent::StreamStart).unwrap();
        writer.write(CborEvent::UInt(1)).unwrap();
        writer.write(CborEvent::StreamEnd).unwrap();
        assert_eq!(
            writer.finish().err().unwrap().kind,
            CborErrorKind::NoProgress
        );
        assert_eq!(
            writer.write(CborEvent::UInt(2)).unwrap_err().kind,
            CborErrorKind::Closed
        );

        struct FlushFails;
        impl std::io::Write for FlushFails {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("flush failure"))
            }
        }
        let mut writer = CborWriter::to_writer(FlushFails, CborEncodeOptions::default()).unwrap();
        writer.write(CborEvent::StreamStart).unwrap();
        writer.write(CborEvent::UInt(1)).unwrap();
        writer.write(CborEvent::StreamEnd).unwrap();
        assert_eq!(writer.finish().err().unwrap().kind, CborErrorKind::IoError);
        assert_eq!(
            writer.write(CborEvent::UInt(2)).unwrap_err().kind,
            CborErrorKind::Closed
        );
    }
}
