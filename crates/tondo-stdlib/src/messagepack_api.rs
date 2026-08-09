//! Public, bounded MessagePack owner.
//!
//! The format kernel in `messagepack.rs` is retained for the hosted bridge.
//! This module owns the source-level API: decoding is driven by an explicit
//! container stack, dynamic maps retain arbitrary ordered keys, and typed
//! serialization uses the common event protocol without requiring callers to
//! depend on a format-specific reflection table.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::fmt;
use std::io::Read;

use crate::serialization::{
    self, Decode, Decoder, Deserialize, Encode, Encoder, Event, MessagePack as MessagePackCodec,
    Raw as RawCodec, SerializationError, Serialize,
};

#[derive(Debug, Clone, PartialEq)]
pub struct MessagePackEntry {
    pub key: MessagePackValue,
    pub value: MessagePackValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePackValue {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    /// The payload is the IEEE-754 bit pattern, preserving NaN payloads and
    /// signed zero across the ordinary encode/decode path.
    Float32(u32),
    /// The payload is the IEEE-754 bit pattern, preserving NaN payloads and
    /// signed zero across the ordinary encode/decode path.
    Float64(u64),
    String(String),
    Binary(Vec<u8>),
    Array(Vec<MessagePackValue>),
    Map(Vec<MessagePackEntry>),
    Ext(MessagePackExt),
}

pub type Value = MessagePackValue;
pub type CommonValue = serialization::Value;

/// A borrowed, input-backed MessagePack view.
///
/// Validation is performed at construction time, but the dynamic value tree
/// is not materialised until `clone_value` is requested.  Keeping the source
/// bytes makes the lifetime and allocation boundary explicit for callers that
/// only need to inspect or forward a validated document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackValueView<'a> {
    input: &'a [u8],
    options: MessagePackDecodeOptions,
}

impl<'a> MessagePackValueView<'a> {
    pub fn bytes(self) -> &'a [u8] {
        self.input
    }

    pub fn clone_value(self) -> Result<MessagePackValue, MessagePackError> {
        decode_value(self.input, self.options)
    }
}

pub type ValueView<'a> = MessagePackValueView<'a>;
pub type Raw = RawCodec<MessagePackCodec>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePackExt {
    pub type_code: i8,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePackTimestamp {
    pub seconds: i64,
    pub nanoseconds: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessagePackPath(Vec<MessagePackPathSegment>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagePackPathSegment {
    ArrayIndex(usize),
    MapEntry(usize),
    MapKey,
    MapValue,
}

impl MessagePackPath {
    pub fn segments(&self) -> &[MessagePackPathSegment] {
        &self.0
    }

    fn push(&mut self, segment: MessagePackPathSegment) {
        self.0.push(segment);
    }
}

impl fmt::Display for MessagePackPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        for segment in &self.0 {
            match segment {
                MessagePackPathSegment::ArrayIndex(index) => write!(formatter, "[{index}]")?,
                MessagePackPathSegment::MapEntry(index) => write!(formatter, "[entry {index}]")?,
                MessagePackPathSegment::MapKey => formatter.write_str("[key]")?,
                MessagePackPathSegment::MapValue => formatter.write_str("[value]")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackErrorKind {
    UnexpectedEof,
    InvalidTag,
    InvalidUtf8,
    InvalidLength,
    NonMinimalEncoding,
    InvalidExtension,
    TypeMismatch,
    DuplicateKey,
    NumberRange,
    DeterministicKeyCollision,
    OutOfOrderKey,
    LimitExceeded,
    IoError,
    TrailingData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePackError {
    pub kind: MessagePackErrorKind,
    pub offset: usize,
    pub path: MessagePackPath,
}

impl MessagePackError {
    fn new(kind: MessagePackErrorKind, offset: usize) -> Self {
        Self {
            kind,
            offset,
            path: MessagePackPath::default(),
        }
    }

    fn with_path(mut self, path: MessagePackPath) -> Self {
        self.path = path;
        self
    }
}

impl fmt::Display for MessagePackError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "MessagePack {:?} at byte {}",
            self.kind, self.offset
        )
    }
}

impl std::error::Error for MessagePackError {}

impl From<SerializationError> for MessagePackError {
    fn from(error: SerializationError) -> Self {
        let kind = match error {
            SerializationError::EndOfInput => MessagePackErrorKind::UnexpectedEof,
            SerializationError::LimitExceeded => MessagePackErrorKind::LimitExceeded,
            SerializationError::DuplicateField => MessagePackErrorKind::DuplicateKey,
            SerializationError::TypeMismatch
            | SerializationError::UnexpectedEvent
            | SerializationError::UnbalancedContainer
            | SerializationError::InvalidContainerLength => MessagePackErrorKind::TypeMismatch,
        };
        MessagePackError::new(kind, 0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackDuplicatePolicy {
    Preserve,
    Reject,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackUnknownExtensionPolicy {
    Preserve,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePackNonMinimalPolicy {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackLimits {
    pub max_document_bytes: usize,
    pub max_depth: usize,
    pub max_array_items: usize,
    pub max_map_pairs: usize,
    pub max_string_bytes: usize,
    pub max_binary_bytes: usize,
    pub max_ext_bytes: usize,
    pub max_events: usize,
    pub max_output_bytes: usize,
}

impl Default for MessagePackLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_array_items: 1_048_576,
            max_map_pairs: 1_048_576,
            max_string_bytes: 64 * 1024 * 1024,
            max_binary_bytes: 64 * 1024 * 1024,
            max_ext_bytes: 64 * 1024 * 1024,
            max_events: 1_000_000,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

impl MessagePackLimits {
    fn valid(self) -> bool {
        self.max_document_bytes > 0
            && self.max_depth > 0
            && self.max_array_items > 0
            && self.max_map_pairs > 0
            && self.max_string_bytes > 0
            && self.max_binary_bytes > 0
            && self.max_ext_bytes > 0
            && self.max_events > 0
            && self.max_output_bytes > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePackDecodeOptions {
    pub limits: MessagePackLimits,
    pub dynamic_map_duplicates: MessagePackDuplicatePolicy,
    pub typed_map_duplicates: MessagePackDuplicatePolicy,
    pub non_minimal: MessagePackNonMinimalPolicy,
    pub unknown_extensions: MessagePackUnknownExtensionPolicy,
}

impl Default for MessagePackDecodeOptions {
    fn default() -> Self {
        Self {
            limits: MessagePackLimits::default(),
            dynamic_map_duplicates: MessagePackDuplicatePolicy::Preserve,
            typed_map_duplicates: MessagePackDuplicatePolicy::Reject,
            non_minimal: MessagePackNonMinimalPolicy::Accept,
            unknown_extensions: MessagePackUnknownExtensionPolicy::Preserve,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MessagePackEncodeOptions {
    pub limits: MessagePackLimits,
    pub deterministic: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MessagePackEvent {
    Nil,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float32(u32),
    Float64(u64),
    String(String),
    Binary(Vec<u8>),
    StartArray(Option<usize>),
    EndArray,
    StartMap(Option<usize>),
    MapKey,
    EndMap,
    Ext(MessagePackExt),
}

#[derive(Debug, Clone, PartialEq)]
enum DecodeFrame {
    Array {
        remaining: usize,
        values: Vec<MessagePackValue>,
    },
    Map {
        remaining: usize,
        entries: Vec<MessagePackEntry>,
        pending_key: Option<MessagePackValue>,
    },
}

/// Required by the owner contract: the parser's state is an explicit stack,
/// so hostile nesting cannot consume the host call stack.
#[allow(dead_code)]
enum Frame {
    Decode(DecodeFrame),
    Events(Vec<MessagePackEvent>),
}

fn error(kind: MessagePackErrorKind, offset: usize) -> MessagePackError {
    MessagePackError::new(kind, offset)
}

fn checked_options(limits: MessagePackLimits) -> Result<(), MessagePackError> {
    limits
        .valid()
        .then_some(())
        .ok_or_else(|| error(MessagePackErrorKind::LimitExceeded, 0))
}

fn take<'a>(
    input: &'a [u8],
    offset: &mut usize,
    count: usize,
) -> Result<&'a [u8], MessagePackError> {
    let end = offset
        .checked_add(count)
        .ok_or_else(|| error(MessagePackErrorKind::InvalidLength, *offset))?;
    let bytes = input
        .get(*offset..end)
        .ok_or_else(|| error(MessagePackErrorKind::UnexpectedEof, *offset))?;
    *offset = end;
    Ok(bytes)
}

fn u8_at(input: &[u8], offset: &mut usize) -> Result<u8, MessagePackError> {
    Ok(take(input, offset, 1)?[0])
}

fn u16_at(input: &[u8], offset: &mut usize) -> Result<u16, MessagePackError> {
    Ok(u16::from_be_bytes(
        take(input, offset, 2)?.try_into().expect("bounded read"),
    ))
}

fn u32_at(input: &[u8], offset: &mut usize) -> Result<u32, MessagePackError> {
    Ok(u32::from_be_bytes(
        take(input, offset, 4)?.try_into().expect("bounded read"),
    ))
}

fn u64_at(input: &[u8], offset: &mut usize) -> Result<u64, MessagePackError> {
    Ok(u64::from_be_bytes(
        take(input, offset, 8)?.try_into().expect("bounded read"),
    ))
}

fn minimal_numeric(
    policy: MessagePackNonMinimalPolicy,
    tag: u8,
    value: i128,
) -> Result<(), MessagePackError> {
    if policy == MessagePackNonMinimalPolicy::Accept {
        return Ok(());
    }
    let minimal = if value >= 0 {
        if value <= 127 {
            0
        } else if value <= u8::MAX as i128 {
            0xcc
        } else if value <= u16::MAX as i128 {
            0xcd
        } else if value <= u32::MAX as i128 {
            0xce
        } else {
            0xcf
        }
    } else if (-32..=-1).contains(&value) {
        0
    } else if i8::try_from(value).is_ok() {
        0xd0
    } else if i16::try_from(value).is_ok() {
        0xd1
    } else if i32::try_from(value).is_ok() {
        0xd2
    } else {
        0xd3
    };
    if tag != minimal {
        return Err(error(MessagePackErrorKind::NonMinimalEncoding, 0));
    }
    Ok(())
}

fn duplicate_map_policy(
    entries: &mut Vec<MessagePackEntry>,
    key: MessagePackValue,
    value: MessagePackValue,
    policy: MessagePackDuplicatePolicy,
    offset: usize,
) -> Result<(), MessagePackError> {
    let duplicate = entries.iter().position(|entry| entry.key == key);
    match (policy, duplicate) {
        (MessagePackDuplicatePolicy::Preserve, _) => entries.push(MessagePackEntry { key, value }),
        (MessagePackDuplicatePolicy::Reject, Some(_)) => {
            return Err(error(MessagePackErrorKind::DuplicateKey, offset));
        }
        (MessagePackDuplicatePolicy::Reject, None) => entries.push(MessagePackEntry { key, value }),
        (MessagePackDuplicatePolicy::First, Some(_)) => {}
        (MessagePackDuplicatePolicy::First, None) => entries.push(MessagePackEntry { key, value }),
        (MessagePackDuplicatePolicy::Last, Some(index)) => entries[index].value = value,
        (MessagePackDuplicatePolicy::Last, None) => entries.push(MessagePackEntry { key, value }),
    }
    Ok(())
}

fn parse_one(
    input: &[u8],
    options: MessagePackDecodeOptions,
    map_policy: MessagePackDuplicatePolicy,
) -> Result<(MessagePackValue, usize), MessagePackError> {
    checked_options(options.limits)?;
    if input.len() > options.limits.max_document_bytes {
        return Err(error(MessagePackErrorKind::LimitExceeded, 0));
    }
    let mut offset = 0;
    let mut stack = Vec::<DecodeFrame>::new();
    let mut current = None;
    let mut events = 0usize;
    loop {
        if let Some(value) = current.take() {
            if let Some(frame) = stack.last_mut() {
                match frame {
                    DecodeFrame::Array { remaining, values } => {
                        values.push(value);
                        *remaining -= 1;
                        if *remaining == 0 {
                            let DecodeFrame::Array { values, .. } = stack.pop().expect("frame")
                            else {
                                unreachable!()
                            };
                            current = Some(MessagePackValue::Array(values));
                        }
                    }
                    DecodeFrame::Map {
                        remaining,
                        entries,
                        pending_key,
                    } => {
                        if let Some(key) = pending_key.take() {
                            duplicate_map_policy(entries, key, value, map_policy, offset)?;
                            *remaining -= 1;
                        } else {
                            *pending_key = Some(value);
                        }
                        if *remaining == 0 {
                            let DecodeFrame::Map { entries, .. } = stack.pop().expect("frame")
                            else {
                                unreachable!()
                            };
                            current = Some(MessagePackValue::Map(entries));
                        }
                    }
                }
                continue;
            }
            return Ok((value, offset));
        }

        events = events
            .checked_add(1)
            .ok_or_else(|| error(MessagePackErrorKind::LimitExceeded, offset))?;
        if events > options.limits.max_events {
            return Err(error(MessagePackErrorKind::LimitExceeded, offset));
        }
        let tag_offset = offset;
        let tag = u8_at(input, &mut offset)?;
        let mut scalar = None;
        let mut container = None;
        match tag {
            0xc0 => scalar = Some(MessagePackValue::Nil),
            0xc2 => scalar = Some(MessagePackValue::Bool(false)),
            0xc3 => scalar = Some(MessagePackValue::Bool(true)),
            0x00..=0x7f => scalar = Some(MessagePackValue::UInt(tag as u64)),
            0xe0..=0xff => scalar = Some(MessagePackValue::Int(tag as i8 as i64)),
            0xcc => {
                let value = u8_at(input, &mut offset)? as u64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::UInt(value));
            }
            0xcd => {
                let value = u16_at(input, &mut offset)? as u64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::UInt(value));
            }
            0xce => {
                let value = u32_at(input, &mut offset)? as u64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::UInt(value));
            }
            0xcf => {
                let value = u64_at(input, &mut offset)?;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::UInt(value));
            }
            0xd0 => {
                let value = u8_at(input, &mut offset)? as i8 as i64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::Int(value));
            }
            0xd1 => {
                let value = u16_at(input, &mut offset)? as i16 as i64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::Int(value));
            }
            0xd2 => {
                let value = u32_at(input, &mut offset)? as i32 as i64;
                minimal_numeric(options.non_minimal, tag, value as i128).map_err(|mut e| {
                    e.offset = tag_offset;
                    e
                })?;
                scalar = Some(MessagePackValue::Int(value));
            }
            0xd3 => {
                let value = u64_at(input, &mut offset)? as i64;
                scalar = Some(MessagePackValue::Int(value));
            }
            0xca => scalar = Some(MessagePackValue::Float32(u32_at(input, &mut offset)?)),
            0xcb => scalar = Some(MessagePackValue::Float64(u64_at(input, &mut offset)?)),
            0xa0..=0xbf => {
                let len = (tag & 0x1f) as usize;
                let bytes = take(input, &mut offset, len)?;
                let text = String::from_utf8(bytes.to_vec())
                    .map_err(|_| error(MessagePackErrorKind::InvalidUtf8, tag_offset))?;
                scalar = Some(MessagePackValue::String(text));
            }
            0xd9 => {
                let len = u8_at(input, &mut offset)? as usize;
                if len < 32 && options.non_minimal == MessagePackNonMinimalPolicy::Reject {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                if len > options.limits.max_string_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
                }
                let text = String::from_utf8(take(input, &mut offset, len)?.to_vec())
                    .map_err(|_| error(MessagePackErrorKind::InvalidUtf8, tag_offset))?;
                scalar = Some(MessagePackValue::String(text));
            }
            0xda => {
                let len = u16_at(input, &mut offset)? as usize;
                if len <= u8::MAX as usize
                    && options.non_minimal == MessagePackNonMinimalPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                if len > options.limits.max_string_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
                }
                let text = String::from_utf8(take(input, &mut offset, len)?.to_vec())
                    .map_err(|_| error(MessagePackErrorKind::InvalidUtf8, tag_offset))?;
                scalar = Some(MessagePackValue::String(text));
            }
            0xdb => {
                let len = usize::try_from(u32_at(input, &mut offset)?)
                    .map_err(|_| error(MessagePackErrorKind::InvalidLength, tag_offset))?;
                if len <= u16::MAX as usize
                    && options.non_minimal == MessagePackNonMinimalPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                if len > options.limits.max_string_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
                }
                let text = String::from_utf8(take(input, &mut offset, len)?.to_vec())
                    .map_err(|_| error(MessagePackErrorKind::InvalidUtf8, tag_offset))?;
                scalar = Some(MessagePackValue::String(text));
            }
            0xc4..=0xc6 => {
                let len = match tag {
                    0xc4 => u8_at(input, &mut offset)? as usize,
                    0xc5 => u16_at(input, &mut offset)? as usize,
                    _ => usize::try_from(u32_at(input, &mut offset)?)
                        .map_err(|_| error(MessagePackErrorKind::InvalidLength, tag_offset))?,
                };
                if len > options.limits.max_binary_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
                }
                scalar = Some(MessagePackValue::Binary(
                    take(input, &mut offset, len)?.to_vec(),
                ));
            }
            0x90..=0x9f => container = Some((true, (tag & 0x0f) as usize)),
            0xdc => {
                let len = u16_at(input, &mut offset)? as usize;
                if len < 16 && options.non_minimal == MessagePackNonMinimalPolicy::Reject {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                container = Some((true, len));
            }
            0xdd => {
                let len = usize::try_from(u32_at(input, &mut offset)?)
                    .map_err(|_| error(MessagePackErrorKind::InvalidLength, tag_offset))?;
                if len <= u16::MAX as usize
                    && options.non_minimal == MessagePackNonMinimalPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                container = Some((true, len));
            }
            0x80..=0x8f => container = Some((false, (tag & 0x0f) as usize)),
            0xde => {
                let len = u16_at(input, &mut offset)? as usize;
                if len < 16 && options.non_minimal == MessagePackNonMinimalPolicy::Reject {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                container = Some((false, len));
            }
            0xdf => {
                let len = usize::try_from(u32_at(input, &mut offset)?)
                    .map_err(|_| error(MessagePackErrorKind::InvalidLength, tag_offset))?;
                if len <= u16::MAX as usize
                    && options.non_minimal == MessagePackNonMinimalPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                container = Some((false, len));
            }
            0xd4..=0xd8 | 0xc7..=0xc9 => {
                let (len, fixed) = match tag {
                    0xd4 => (1, true),
                    0xd5 => (2, true),
                    0xd6 => (4, true),
                    0xd7 => (8, true),
                    0xd8 => (16, true),
                    0xc7 => (u8_at(input, &mut offset)? as usize, false),
                    0xc8 => (u16_at(input, &mut offset)? as usize, false),
                    _ => (
                        usize::try_from(u32_at(input, &mut offset)?)
                            .map_err(|_| error(MessagePackErrorKind::InvalidLength, tag_offset))?,
                        false,
                    ),
                };
                if len > options.limits.max_ext_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
                }
                if !fixed
                    && [1, 2, 4, 8, 16].contains(&len)
                    && options.non_minimal == MessagePackNonMinimalPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::NonMinimalEncoding, tag_offset));
                }
                let type_code = u8_at(input, &mut offset)? as i8;
                if type_code != -1
                    && options.unknown_extensions == MessagePackUnknownExtensionPolicy::Reject
                {
                    return Err(error(MessagePackErrorKind::InvalidExtension, tag_offset));
                }
                scalar = Some(MessagePackValue::Ext(MessagePackExt {
                    type_code,
                    payload: take(input, &mut offset, len)?.to_vec(),
                }));
            }
            _ => return Err(error(MessagePackErrorKind::InvalidTag, tag_offset)),
        }
        if let Some(value) = scalar {
            current = Some(value);
            continue;
        }
        let (array, count) = container.expect("tag classified");
        if stack.len() >= options.limits.max_depth {
            return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
        }
        if array {
            if count > options.limits.max_array_items {
                return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
            }
            if count == 0 {
                current = Some(MessagePackValue::Array(Vec::new()));
            } else {
                stack.push(DecodeFrame::Array {
                    remaining: count,
                    values: Vec::with_capacity(count),
                });
            }
        } else {
            if count > options.limits.max_map_pairs {
                return Err(error(MessagePackErrorKind::LimitExceeded, tag_offset));
            }
            if count == 0 {
                current = Some(MessagePackValue::Map(Vec::new()));
            } else {
                stack.push(DecodeFrame::Map {
                    remaining: count,
                    entries: Vec::with_capacity(count),
                    pending_key: None,
                });
            }
        }
    }
}

fn ensure_single(
    value: Result<(MessagePackValue, usize), MessagePackError>,
    input: &[u8],
) -> Result<MessagePackValue, MessagePackError> {
    let (value, offset) = value?;
    if offset != input.len() {
        return Err(error(MessagePackErrorKind::TrailingData, offset));
    }
    Ok(value)
}

fn append_output(output: &mut Vec<u8>, bytes: &[u8], limit: usize) -> Result<(), MessagePackError> {
    let end = output
        .len()
        .checked_add(bytes.len())
        .ok_or_else(|| error(MessagePackErrorKind::LimitExceeded, output.len()))?;
    if end > limit {
        return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
    }
    output.extend_from_slice(bytes);
    Ok(())
}

fn length_prefix(
    kind: u8,
    len: usize,
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<(), MessagePackError> {
    let mut prefix = Vec::with_capacity(5);
    match kind {
        0 => {
            if len < 16 {
                prefix.push(0x90 | len as u8);
            } else if len <= u16::MAX as usize {
                prefix.push(0xdc);
                prefix.extend_from_slice(&(len as u16).to_be_bytes());
            } else {
                prefix.push(0xdd);
                prefix.extend_from_slice(&(len as u32).to_be_bytes());
            }
        }
        1 => {
            if len < 16 {
                prefix.push(0x80 | len as u8);
            } else if len <= u16::MAX as usize {
                prefix.push(0xde);
                prefix.extend_from_slice(&(len as u16).to_be_bytes());
            } else {
                prefix.push(0xdf);
                prefix.extend_from_slice(&(len as u32).to_be_bytes());
            }
        }
        2 => {
            if len < 32 {
                prefix.push(0xa0 | len as u8);
            } else if len <= u8::MAX as usize {
                prefix.extend([0xd9, len as u8]);
            } else if len <= u16::MAX as usize {
                prefix.push(0xda);
                prefix.extend_from_slice(&(len as u16).to_be_bytes());
            } else {
                prefix.push(0xdb);
                prefix.extend_from_slice(&(len as u32).to_be_bytes());
            }
        }
        3 => {
            if len <= u8::MAX as usize {
                prefix.extend([0xc4, len as u8]);
            } else if len <= u16::MAX as usize {
                prefix.push(0xc5);
                prefix.extend_from_slice(&(len as u16).to_be_bytes());
            } else {
                prefix.push(0xc6);
                prefix.extend_from_slice(&(len as u32).to_be_bytes());
            }
        }
        _ => unreachable!(),
    }
    append_output(output, &prefix, limit)
}

enum EncodeTask {
    Value(MessagePackValue),
}

fn encode_inner(
    value: &MessagePackValue,
    options: MessagePackEncodeOptions,
) -> Result<Vec<u8>, MessagePackError> {
    checked_options(options.limits)?;
    let mut output = Vec::new();
    let mut tasks = vec![EncodeTask::Value(if options.deterministic {
        canonical_value(value, options.limits.max_depth)?
    } else {
        value.clone()
    })];
    while let Some(EncodeTask::Value(value)) = tasks.pop() {
        match value {
            MessagePackValue::Nil => {
                append_output(&mut output, &[0xc0], options.limits.max_output_bytes)?
            }
            MessagePackValue::Bool(false) => {
                append_output(&mut output, &[0xc2], options.limits.max_output_bytes)?
            }
            MessagePackValue::Bool(true) => {
                append_output(&mut output, &[0xc3], options.limits.max_output_bytes)?
            }
            MessagePackValue::Int(value) => {
                encode_int(value, &mut output, options.limits.max_output_bytes)?
            }
            MessagePackValue::UInt(value) => {
                encode_uint(value, &mut output, options.limits.max_output_bytes)?
            }
            MessagePackValue::Float32(bits) => {
                append_output(&mut output, &[0xca], options.limits.max_output_bytes)?;
                append_output(
                    &mut output,
                    &bits.to_be_bytes(),
                    options.limits.max_output_bytes,
                )?;
            }
            MessagePackValue::Float64(bits) => {
                append_output(&mut output, &[0xcb], options.limits.max_output_bytes)?;
                append_output(
                    &mut output,
                    &bits.to_be_bytes(),
                    options.limits.max_output_bytes,
                )?;
            }
            MessagePackValue::String(text) => {
                if text.len() > options.limits.max_string_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
                }
                length_prefix(2, text.len(), &mut output, options.limits.max_output_bytes)?;
                append_output(
                    &mut output,
                    text.as_bytes(),
                    options.limits.max_output_bytes,
                )?;
            }
            MessagePackValue::Binary(bytes) => {
                if bytes.len() > options.limits.max_binary_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
                }
                length_prefix(3, bytes.len(), &mut output, options.limits.max_output_bytes)?;
                append_output(&mut output, &bytes, options.limits.max_output_bytes)?;
            }
            MessagePackValue::Array(values) => {
                if values.len() > options.limits.max_array_items {
                    return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
                }
                length_prefix(
                    0,
                    values.len(),
                    &mut output,
                    options.limits.max_output_bytes,
                )?;
                for child in values.into_iter().rev() {
                    tasks.push(EncodeTask::Value(child));
                }
            }
            MessagePackValue::Map(entries) => {
                if entries.len() > options.limits.max_map_pairs {
                    return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
                }
                length_prefix(
                    1,
                    entries.len(),
                    &mut output,
                    options.limits.max_output_bytes,
                )?;
                for entry in entries.into_iter().rev() {
                    tasks.push(EncodeTask::Value(entry.value));
                    tasks.push(EncodeTask::Value(entry.key));
                }
            }
            MessagePackValue::Ext(ext) => {
                if ext.payload.len() > options.limits.max_ext_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, output.len()));
                }
                encode_ext(&ext, &mut output, options.limits.max_output_bytes)?
            }
        }
    }
    Ok(output)
}

fn encode_int(value: i64, output: &mut Vec<u8>, limit: usize) -> Result<(), MessagePackError> {
    if (0..=127).contains(&value) {
        return append_output(output, &[value as u8], limit);
    }
    if (-32..=-1).contains(&value) {
        return append_output(output, &[value as i8 as u8], limit);
    }
    if i8::try_from(value).is_ok() {
        append_output(output, &[0xd0, value as i8 as u8], limit)
    } else if i16::try_from(value).is_ok() {
        append_output(output, &[0xd1], limit)?;
        append_output(output, &(value as i16).to_be_bytes(), limit)
    } else if i32::try_from(value).is_ok() {
        append_output(output, &[0xd2], limit)?;
        append_output(output, &(value as i32).to_be_bytes(), limit)
    } else {
        append_output(output, &[0xd3], limit)?;
        append_output(output, &value.to_be_bytes(), limit)
    }
}

fn encode_uint(value: u64, output: &mut Vec<u8>, limit: usize) -> Result<(), MessagePackError> {
    if value <= 127 {
        return append_output(output, &[value as u8], limit);
    }
    if u8::try_from(value).is_ok() {
        append_output(output, &[0xcc, value as u8], limit)
    } else if u16::try_from(value).is_ok() {
        append_output(output, &[0xcd], limit)?;
        append_output(output, &(value as u16).to_be_bytes(), limit)
    } else if u32::try_from(value).is_ok() {
        append_output(output, &[0xce], limit)?;
        append_output(output, &(value as u32).to_be_bytes(), limit)
    } else {
        append_output(output, &[0xcf], limit)?;
        append_output(output, &value.to_be_bytes(), limit)
    }
}

fn encode_ext(
    ext: &MessagePackExt,
    output: &mut Vec<u8>,
    limit: usize,
) -> Result<(), MessagePackError> {
    let len = ext.payload.len();
    let fixed = match len {
        1 => Some(0xd4),
        2 => Some(0xd5),
        4 => Some(0xd6),
        8 => Some(0xd7),
        16 => Some(0xd8),
        _ => None,
    };
    if let Some(tag) = fixed {
        append_output(output, &[tag, ext.type_code as u8], limit)?;
    } else if len <= u8::MAX as usize {
        append_output(output, &[0xc7, len as u8, ext.type_code as u8], limit)?;
    } else if len <= u16::MAX as usize {
        append_output(output, &[0xc8], limit)?;
        append_output(output, &(len as u16).to_be_bytes(), limit)?;
        append_output(output, &[ext.type_code as u8], limit)?;
    } else {
        append_output(output, &[0xc9], limit)?;
        append_output(output, &(len as u32).to_be_bytes(), limit)?;
        append_output(output, &[ext.type_code as u8], limit)?;
    }
    append_output(output, &ext.payload, limit)
}

fn canonical_value(
    value: &MessagePackValue,
    depth: usize,
) -> Result<MessagePackValue, MessagePackError> {
    if depth == 0 {
        return Err(error(MessagePackErrorKind::LimitExceeded, 0));
    }
    Ok(match value {
        MessagePackValue::Array(values) => MessagePackValue::Array(
            values
                .iter()
                .map(|v| canonical_value(v, depth - 1))
                .collect::<Result<Vec<_>, _>>()?,
        ),
        MessagePackValue::Map(entries) => {
            let mut ordered = entries
                .iter()
                .map(|entry| {
                    let key = canonical_value(&entry.key, depth - 1)?;
                    let value = canonical_value(&entry.value, depth - 1)?;
                    let bytes = encode_inner(
                        &key,
                        MessagePackEncodeOptions {
                            limits: MessagePackLimits::default(),
                            deterministic: false,
                        },
                    )?;
                    Ok((bytes, MessagePackEntry { key, value }))
                })
                .collect::<Result<Vec<_>, MessagePackError>>()?;
            ordered.sort_by(|a, b| a.0.cmp(&b.0));
            if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
                return Err(error(MessagePackErrorKind::DeterministicKeyCollision, 0));
            }
            MessagePackValue::Map(ordered.into_iter().map(|(_, entry)| entry).collect())
        }
        MessagePackValue::Float32(bits) if f32::from_bits(*bits).is_nan() => {
            MessagePackValue::Float32(0x7fc0_0000)
        }
        MessagePackValue::Float64(bits) if f64::from_bits(*bits).is_nan() => {
            MessagePackValue::Float64(0x7ff8_0000_0000_0000)
        }
        MessagePackValue::Float64(bits) => {
            let value = f64::from_bits(*bits);
            let narrowed = value as f32;
            if value.is_finite()
                && (narrowed as f64) == value
                && !(value == 0.0 && value.is_sign_negative() != narrowed.is_sign_negative())
            {
                MessagePackValue::Float32(narrowed.to_bits())
            } else {
                MessagePackValue::Float64(*bits)
            }
        }
        _ => value.clone(),
    })
}

enum EventTask {
    Value(MessagePackValue),
    EndArray,
    EndMap,
    MapKey,
}

fn to_events(
    value: &MessagePackValue,
    limits: MessagePackLimits,
) -> Result<Vec<MessagePackEvent>, MessagePackError> {
    let mut events = Vec::new();
    let mut values = vec![EventTask::Value(value.clone())];
    while let Some(task) = values.pop() {
        if events.len() >= limits.max_events {
            return Err(error(MessagePackErrorKind::LimitExceeded, events.len()));
        }
        match task {
            EventTask::EndArray => events.push(MessagePackEvent::EndArray),
            EventTask::EndMap => events.push(MessagePackEvent::EndMap),
            EventTask::MapKey => events.push(MessagePackEvent::MapKey),
            EventTask::Value(value) => match value {
                MessagePackValue::Nil => events.push(MessagePackEvent::Nil),
                MessagePackValue::Bool(v) => events.push(MessagePackEvent::Bool(v)),
                MessagePackValue::Int(v) => events.push(MessagePackEvent::Int(v)),
                MessagePackValue::UInt(v) => events.push(MessagePackEvent::UInt(v)),
                MessagePackValue::Float32(v) => events.push(MessagePackEvent::Float32(v)),
                MessagePackValue::Float64(v) => events.push(MessagePackEvent::Float64(v)),
                MessagePackValue::String(v) => events.push(MessagePackEvent::String(v)),
                MessagePackValue::Binary(v) => events.push(MessagePackEvent::Binary(v)),
                MessagePackValue::Ext(v) => events.push(MessagePackEvent::Ext(v)),
                MessagePackValue::Array(children) => {
                    events.push(MessagePackEvent::StartArray(Some(children.len())));
                    values.push(EventTask::EndArray);
                    for child in children.into_iter().rev() {
                        values.push(EventTask::Value(child));
                    }
                }
                MessagePackValue::Map(entries) => {
                    events.push(MessagePackEvent::StartMap(Some(entries.len())));
                    values.push(EventTask::EndMap);
                    for entry in entries.into_iter().rev() {
                        values.push(EventTask::Value(entry.value));
                        values.push(EventTask::Value(entry.key));
                        values.push(EventTask::MapKey);
                    }
                }
            },
        }
    }
    Ok(events)
}

#[derive(Clone)]
pub struct MessagePackReader<'a> {
    input: Cow<'a, [u8]>,
    options: MessagePackDecodeOptions,
    offset: usize,
    events: VecDeque<MessagePackEvent>,
    loaded: bool,
    emitted_none: bool,
    terminal: Option<MessagePackError>,
}

impl<'a> MessagePackReader<'a> {
    pub fn from_bytes(
        input: &'a [u8],
        options: MessagePackDecodeOptions,
    ) -> Result<Self, MessagePackError> {
        checked_options(options.limits)?;
        if input.len() > options.limits.max_document_bytes {
            return Err(error(MessagePackErrorKind::LimitExceeded, 0));
        }
        Ok(Self {
            input: Cow::Borrowed(input),
            options,
            offset: 0,
            events: VecDeque::new(),
            loaded: false,
            emitted_none: false,
            terminal: None,
        })
    }

    pub fn from_chunks<I, B>(
        chunks: I,
        options: MessagePackDecodeOptions,
    ) -> Result<MessagePackReader<'static>, MessagePackError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let mut bytes = Vec::new();
        for chunk in chunks {
            let chunk = chunk.as_ref();
            let end = bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| error(MessagePackErrorKind::LimitExceeded, bytes.len()))?;
            if end > options.limits.max_document_bytes {
                return Err(error(MessagePackErrorKind::LimitExceeded, bytes.len()));
            }
            bytes.extend_from_slice(chunk);
        }
        checked_options(options.limits)?;
        Ok(MessagePackReader {
            input: Cow::Owned(bytes),
            options,
            offset: 0,
            events: VecDeque::new(),
            loaded: false,
            emitted_none: false,
            terminal: None,
        })
    }

    pub fn from_reader<R: Read>(
        input: R,
        options: MessagePackDecodeOptions,
    ) -> Result<MessagePackReader<'static>, MessagePackError> {
        checked_options(options.limits)?;
        let mut bytes = Vec::new();
        input
            .take(options.limits.max_document_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| error(MessagePackErrorKind::IoError, 0))?;
        if bytes.len() > options.limits.max_document_bytes {
            return Err(error(MessagePackErrorKind::LimitExceeded, 0));
        }
        MessagePackReader::from_chunks([bytes], options)
    }

    fn fail<T>(&mut self, error: MessagePackError) -> Result<T, MessagePackError> {
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn load(&mut self) -> Result<(), MessagePackError> {
        let (value, used) = parse_one(
            &self.input[self.offset..],
            self.options,
            self.options.dynamic_map_duplicates,
        )
        .map_err(|mut e| {
            e.offset += self.offset;
            e
        })?;
        let events = to_events(&value, self.options.limits)?;
        self.offset += used;
        self.events = events.into_iter().collect();
        self.loaded = true;
        Ok(())
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<MessagePackEvent>, MessagePackError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.emitted_none {
            return Ok(None);
        }
        if !self.loaded
            && let Err(error) = self.load()
        {
            return self.fail(error);
        }
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }
        if self.offset != self.input.len() {
            return self.fail(error(MessagePackErrorKind::TrailingData, self.offset));
        }
        self.emitted_none = true;
        Ok(None)
    }

    pub fn own(&mut self, event: MessagePackEvent) -> Result<MessagePackEvent, MessagePackError> {
        if self.terminal.is_some() {
            return self.fail(error(MessagePackErrorKind::TypeMismatch, self.offset));
        }
        Ok(event)
    }

    pub fn finish(&mut self) -> Result<(), MessagePackError> {
        while self.next()?.is_some() {}
        Ok(())
    }
}

impl Raw {
    /// Validate and retain an exact MessagePack document without materialising
    /// its dynamic value tree.
    pub fn from_bytes(
        input: &[u8],
        options: MessagePackDecodeOptions,
    ) -> Result<Self, MessagePackError> {
        let mut reader = MessagePackReader::from_bytes(input, options)?;
        reader.finish()?;
        Ok(RawCodec::from_validated(input.to_vec()))
    }
}

#[derive(Debug)]
enum WriterFrame {
    Array(Vec<MessagePackValue>),
    Map {
        entries: Vec<MessagePackEntry>,
        pending_key: Option<MessagePackValue>,
        key_phase: bool,
        last_key_bytes: Option<Vec<u8>>,
    },
}

pub struct MessagePackWriter {
    options: MessagePackEncodeOptions,
    stack: Vec<WriterFrame>,
    root: Option<MessagePackValue>,
    terminal: Option<MessagePackError>,
    finished: bool,
}

impl MessagePackWriter {
    pub fn new(options: MessagePackEncodeOptions) -> Self {
        Self {
            options,
            stack: Vec::new(),
            root: None,
            terminal: None,
            finished: false,
        }
    }

    pub fn to_writer(options: MessagePackEncodeOptions) -> Self {
        Self::new(options)
    }

    fn attach(&mut self, value: MessagePackValue) -> Result<(), MessagePackError> {
        if let Some(frame) = self.stack.last_mut() {
            match frame {
                WriterFrame::Array(values) => {
                    if values.len() >= self.options.limits.max_array_items {
                        return Err(error(MessagePackErrorKind::LimitExceeded, values.len()));
                    }
                    values.push(value);
                }
                WriterFrame::Map {
                    entries,
                    pending_key,
                    key_phase,
                    last_key_bytes,
                } => {
                    if *key_phase {
                        if self.options.deterministic {
                            let key_bytes = encode_inner(
                                &value,
                                MessagePackEncodeOptions {
                                    deterministic: true,
                                    ..self.options
                                },
                            )?;
                            if let Some(previous) = last_key_bytes {
                                match key_bytes.cmp(previous) {
                                    std::cmp::Ordering::Less => {
                                        return Err(error(
                                            MessagePackErrorKind::OutOfOrderKey,
                                            entries.len(),
                                        ));
                                    }
                                    std::cmp::Ordering::Equal => {
                                        return Err(error(
                                            MessagePackErrorKind::DeterministicKeyCollision,
                                            entries.len(),
                                        ));
                                    }
                                    std::cmp::Ordering::Greater => {}
                                }
                            }
                            *last_key_bytes = Some(key_bytes);
                        }
                        *pending_key = Some(value);
                        *key_phase = false;
                    } else {
                        let key = pending_key.take().ok_or_else(|| {
                            error(MessagePackErrorKind::TypeMismatch, entries.len())
                        })?;
                        entries.push(MessagePackEntry { key, value });
                        *key_phase = true;
                        if entries.len() > self.options.limits.max_map_pairs {
                            return Err(error(MessagePackErrorKind::LimitExceeded, entries.len()));
                        }
                    }
                }
            }
        } else if self.root.is_some() {
            return Err(error(MessagePackErrorKind::TrailingData, 0));
        } else {
            self.root = Some(value);
        }
        Ok(())
    }

    fn inner_write(&mut self, event: MessagePackEvent) -> Result<(), MessagePackError> {
        checked_options(self.options.limits)?;
        if self.finished {
            return Err(error(MessagePackErrorKind::TrailingData, 0));
        }
        match event {
            MessagePackEvent::Nil => self.attach(MessagePackValue::Nil),
            MessagePackEvent::Bool(v) => self.attach(MessagePackValue::Bool(v)),
            MessagePackEvent::Int(v) => self.attach(MessagePackValue::Int(v)),
            MessagePackEvent::UInt(v) => self.attach(MessagePackValue::UInt(v)),
            MessagePackEvent::Float32(v) => self.attach(MessagePackValue::Float32(v)),
            MessagePackEvent::Float64(v) => self.attach(MessagePackValue::Float64(v)),
            MessagePackEvent::String(v) => {
                if v.len() > self.options.limits.max_output_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, 0));
                }
                self.attach(MessagePackValue::String(v))
            }
            MessagePackEvent::Binary(v) => {
                if v.len() > self.options.limits.max_output_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, 0));
                }
                self.attach(MessagePackValue::Binary(v))
            }
            MessagePackEvent::Ext(v) => {
                if v.payload.len() > self.options.limits.max_ext_bytes {
                    return Err(error(MessagePackErrorKind::LimitExceeded, 0));
                }
                self.attach(MessagePackValue::Ext(v))
            }
            MessagePackEvent::StartArray(_) => {
                if self.stack.len() >= self.options.limits.max_depth {
                    return Err(error(MessagePackErrorKind::LimitExceeded, self.stack.len()));
                }
                self.stack.push(WriterFrame::Array(Vec::new()));
                Ok(())
            }
            MessagePackEvent::EndArray => {
                let WriterFrame::Array(values) = self
                    .stack
                    .pop()
                    .ok_or_else(|| error(MessagePackErrorKind::TypeMismatch, 0))?
                else {
                    return Err(error(MessagePackErrorKind::TypeMismatch, 0));
                };
                self.attach(MessagePackValue::Array(values))
            }
            MessagePackEvent::StartMap(_) => {
                if self.stack.len() >= self.options.limits.max_depth {
                    return Err(error(MessagePackErrorKind::LimitExceeded, self.stack.len()));
                }
                self.stack.push(WriterFrame::Map {
                    entries: Vec::new(),
                    pending_key: None,
                    key_phase: true,
                    last_key_bytes: None,
                });
                Ok(())
            }
            MessagePackEvent::MapKey => match self.stack.last() {
                Some(WriterFrame::Map {
                    key_phase: true,
                    pending_key: None,
                    ..
                }) => Ok(()),
                _ => Err(error(MessagePackErrorKind::TypeMismatch, 0)),
            },
            MessagePackEvent::EndMap => {
                let WriterFrame::Map {
                    entries,
                    pending_key,
                    key_phase,
                    ..
                } = self
                    .stack
                    .pop()
                    .ok_or_else(|| error(MessagePackErrorKind::TypeMismatch, 0))?
                else {
                    return Err(error(MessagePackErrorKind::TypeMismatch, 0));
                };
                if pending_key.is_some() || !key_phase {
                    return Err(error(MessagePackErrorKind::TypeMismatch, 0));
                }
                self.attach(MessagePackValue::Map(entries))
            }
        }
    }

    pub fn write(&mut self, event: MessagePackEvent) -> Result<(), MessagePackError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if let Err(error) = self.inner_write(event) {
            self.terminal = Some(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, MessagePackError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return Err(error(MessagePackErrorKind::TrailingData, 0));
        }
        if !self.stack.is_empty() || self.root.is_none() {
            let error = error(MessagePackErrorKind::TypeMismatch, 0);
            self.terminal = Some(error.clone());
            return Err(error);
        }
        self.finished = true;
        encode_inner(self.root.as_ref().expect("root"), self.options)
    }
}

fn serialization_event(event: &Event) -> Result<MessagePackEvent, MessagePackError> {
    Ok(match event {
        Event::Null => MessagePackEvent::Nil,
        Event::Bool(v) => MessagePackEvent::Bool(*v),
        Event::Int(v) => MessagePackEvent::Int(
            i64::try_from(*v).map_err(|_| error(MessagePackErrorKind::NumberRange, 0))?,
        ),
        Event::UInt(v) => MessagePackEvent::UInt(
            u64::try_from(*v).map_err(|_| error(MessagePackErrorKind::NumberRange, 0))?,
        ),
        Event::Float32(v) => MessagePackEvent::Float32(*v),
        Event::Float64(v) => MessagePackEvent::Float64(*v),
        Event::Float(v) => MessagePackEvent::Float64(v.to_bits()),
        Event::String(v) => MessagePackEvent::String(v.clone()),
        Event::Bytes(v) => MessagePackEvent::Binary(v.clone()),
        Event::StartArray(length) => MessagePackEvent::StartArray(*length),
        Event::EndArray => MessagePackEvent::EndArray,
        Event::StartMap(length) => MessagePackEvent::StartMap(*length),
        Event::MapKey => MessagePackEvent::MapKey,
        Event::EndMap => MessagePackEvent::EndMap,
        Event::StartRecord { .. }
        | Event::Field(_)
        | Event::EndRecord
        | Event::StartEnum { .. }
        | Event::EndEnum => return Err(error(MessagePackErrorKind::TypeMismatch, 0)),
    })
}

fn value_event(value: &MessagePackValue) -> Event {
    match value {
        MessagePackValue::Nil => Event::Null,
        MessagePackValue::Bool(v) => Event::Bool(*v),
        MessagePackValue::Int(v) => Event::Int(i128::from(*v)),
        MessagePackValue::UInt(v) => Event::UInt(u128::from(*v)),
        MessagePackValue::Float32(v) => Event::Float32(*v),
        MessagePackValue::Float64(v) => Event::Float64(*v),
        MessagePackValue::String(v) => Event::String(v.clone()),
        MessagePackValue::Binary(v) => Event::Bytes(v.clone()),
        MessagePackValue::Array(_) | MessagePackValue::Map(_) | MessagePackValue::Ext(_) => {
            Event::Null
        }
    }
}

fn value_to_serialization_events(
    value: &MessagePackValue,
    limits: MessagePackLimits,
) -> Result<Vec<Event>, MessagePackError> {
    let events = to_events(value, limits)?;
    let mut output = Vec::new();
    let mut pending_map_key = false;
    for event in events {
        match event {
            MessagePackEvent::StartArray(length) => output.push(Event::StartArray(length)),
            MessagePackEvent::EndArray => output.push(Event::EndArray),
            MessagePackEvent::StartMap(length) => {
                pending_map_key = false;
                output.push(Event::StartMap(length));
            }
            MessagePackEvent::MapKey => {
                pending_map_key = true;
                output.push(Event::MapKey);
            }
            MessagePackEvent::EndMap => output.push(Event::EndMap),
            MessagePackEvent::Ext(_) => return Err(error(MessagePackErrorKind::TypeMismatch, 0)),
            scalar => {
                let _ = pending_map_key;
                output.push(value_event(&event_to_value_for_event(&scalar)?));
            }
        }
    }
    Ok(output)
}

fn event_to_value_for_event(
    event: &MessagePackEvent,
) -> Result<MessagePackValue, MessagePackError> {
    Ok(match event {
        MessagePackEvent::Nil => MessagePackValue::Nil,
        MessagePackEvent::Bool(v) => MessagePackValue::Bool(*v),
        MessagePackEvent::Int(v) => MessagePackValue::Int(*v),
        MessagePackEvent::UInt(v) => MessagePackValue::UInt(*v),
        MessagePackEvent::Float32(v) => MessagePackValue::Float32(*v),
        MessagePackEvent::Float64(v) => MessagePackValue::Float64(*v),
        MessagePackEvent::String(v) => MessagePackValue::String(v.clone()),
        MessagePackEvent::Binary(v) => MessagePackValue::Binary(v.clone()),
        _ => return Err(error(MessagePackErrorKind::TypeMismatch, 0)),
    })
}

pub fn decode_value(
    input: &[u8],
    options: MessagePackDecodeOptions,
) -> Result<MessagePackValue, MessagePackError> {
    ensure_single(
        parse_one(input, options, options.dynamic_map_duplicates),
        input,
    )
}

/// Parse a complete dynamic document with the caller's explicit policies.
/// This is the Rust bridge for the source-level `std.messagepack.parse` API.
pub fn parse(input: &[u8], options: MessagePackDecodeOptions) -> Result<Value, MessagePackError> {
    decode_value(input, options)
}

/// Validate a complete document and return an input-backed view without
/// materialising its dynamic value tree.
pub fn parse_view(
    input: &[u8],
    options: MessagePackDecodeOptions,
) -> Result<ValueView<'_>, MessagePackError> {
    validate(input, options)?;
    Ok(MessagePackValueView { input, options })
}

/// Validate and retain exact wire bytes as an opaque MessagePack value.
pub fn raw(input: &[u8], options: MessagePackDecodeOptions) -> Result<Raw, MessagePackError> {
    Raw::from_bytes(input, options)
}

/// Rust-safe bridge for the source-level `unsafe rawUnchecked` operation.
/// The Tondo surface exposes this only inside an `unsafe` block; the host
/// representation itself does not require Rust `unsafe` code.
pub fn raw_unchecked(input: &[u8]) -> Raw {
    RawCodec::from_unchecked(input.to_vec())
}

#[allow(non_snake_case)]
pub fn decodeValue(
    input: &[u8],
    options: MessagePackDecodeOptions,
) -> Result<MessagePackValue, MessagePackError> {
    decode_value(input, options)
}

pub fn encode_value(
    value: &MessagePackValue,
    options: MessagePackEncodeOptions,
) -> Result<Vec<u8>, MessagePackError> {
    encode_inner(value, options)
}

#[allow(non_snake_case)]
pub fn encodeValue(
    value: &MessagePackValue,
    options: MessagePackEncodeOptions,
) -> Result<Vec<u8>, MessagePackError> {
    encode_value(value, options)
}

pub fn encode_typed<T: Serialize>(
    value: &T,
    options: MessagePackEncodeOptions,
) -> Result<Vec<u8>, MessagePackError> {
    let events = serialization::serialize_value(
        value,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_events,
            max_bytes: options.limits.max_output_bytes,
            max_container_items: options.limits.max_array_items,
        },
    )
    .map_err(|_| error(MessagePackErrorKind::TypeMismatch, 0))?;
    let mut writer = MessagePackWriter::new(options);
    for event in &events {
        writer.write(serialization_event(event)?)?;
    }
    writer.finish()
}

pub fn decode_typed<T: Deserialize>(
    input: &[u8],
    options: MessagePackDecodeOptions,
) -> Result<T, MessagePackError> {
    let value = ensure_single(
        parse_one(input, options, options.typed_map_duplicates),
        input,
    )?;
    let events = value_to_serialization_events(&value, options.limits)?;
    serialization::deserialize_value(
        &events,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_events,
            max_bytes: options.limits.max_document_bytes,
            max_container_items: options.limits.max_array_items,
        },
    )
    .map_err(|_| error(MessagePackErrorKind::TypeMismatch, 0))
}

pub fn encode(value: &MessagePackValue) -> Vec<u8> {
    encode_inner(value, MessagePackEncodeOptions::default()).unwrap_or_default()
}

pub fn decode(input: &[u8]) -> Result<MessagePackValue, MessagePackError> {
    decode_value(input, MessagePackDecodeOptions::default())
}

pub fn encode_deterministic(value: &MessagePackValue) -> Result<Vec<u8>, MessagePackError> {
    encode_inner(
        value,
        MessagePackEncodeOptions {
            deterministic: true,
            ..Default::default()
        },
    )
}

pub fn validate(input: &[u8], options: MessagePackDecodeOptions) -> Result<(), MessagePackError> {
    decode_value(input, options).map(|_| ())
}

pub fn encode_deterministic_with_limits(
    value: &MessagePackValue,
    limits: MessagePackLimits,
) -> Result<Vec<u8>, MessagePackError> {
    encode_inner(
        value,
        MessagePackEncodeOptions {
            limits,
            deterministic: true,
        },
    )
}

#[allow(non_snake_case)]
pub fn encodeDeterministic(
    value: &MessagePackValue,
    limits: MessagePackLimits,
) -> Result<Vec<u8>, MessagePackError> {
    encode_deterministic_with_limits(value, limits)
}

impl MessagePackTimestamp {
    pub fn from_ext(value: &MessagePackExt) -> Result<Self, MessagePackError> {
        if value.type_code != -1 {
            return Err(error(MessagePackErrorKind::InvalidExtension, 0));
        }
        match value.payload.len() {
            4 => Ok(Self {
                seconds: i64::from(u32::from_be_bytes(
                    value.payload[..4].try_into().expect("length"),
                )),
                nanoseconds: 0,
            }),
            8 => {
                let raw = u64::from_be_bytes(value.payload[..8].try_into().expect("length"));
                Ok(Self {
                    seconds: (raw & 0x0000_0000_3fff_ffff) as i64,
                    nanoseconds: (raw >> 34) as i32,
                })
            }
            12 => {
                let nanos = u32::from_be_bytes(value.payload[..4].try_into().expect("length"));
                if nanos >= 1_000_000_000 {
                    return Err(error(MessagePackErrorKind::InvalidExtension, 0));
                }
                Ok(Self {
                    seconds: i64::from_be_bytes(value.payload[4..12].try_into().expect("length")),
                    nanoseconds: nanos as i32,
                })
            }
            _ => Err(error(MessagePackErrorKind::InvalidExtension, 0)),
        }
    }

    pub fn to_ext(&self) -> Result<MessagePackExt, MessagePackError> {
        if !(0..1_000_000_000).contains(&self.nanoseconds) {
            return Err(error(MessagePackErrorKind::InvalidExtension, 0));
        }
        if self.seconds >= 0 && self.seconds <= u32::MAX as i64 && self.nanoseconds == 0 {
            return Ok(MessagePackExt {
                type_code: -1,
                payload: (self.seconds as u32).to_be_bytes().to_vec(),
            });
        }
        if self.seconds >= 0 && self.seconds < (1_i64 << 34) {
            let raw = ((self.nanoseconds as u64) << 34) | self.seconds as u64;
            return Ok(MessagePackExt {
                type_code: -1,
                payload: raw.to_be_bytes().to_vec(),
            });
        }
        let mut payload = self.nanoseconds.to_be_bytes().to_vec();
        payload.extend_from_slice(&self.seconds.to_be_bytes());
        Ok(MessagePackExt {
            type_code: -1,
            payload,
        })
    }

    #[allow(non_snake_case)]
    pub fn fromExt(value: &MessagePackExt) -> Result<Self, MessagePackError> {
        Self::from_ext(value)
    }
    #[allow(non_snake_case)]
    pub fn toExt(&self) -> Result<MessagePackExt, MessagePackError> {
        self.to_ext()
    }
}

fn messagepack_event_to_common(event: MessagePackEvent) -> Result<Event, MessagePackError> {
    Ok(match event {
        MessagePackEvent::Nil => Event::Null,
        MessagePackEvent::Bool(value) => Event::Bool(value),
        MessagePackEvent::Int(value) => Event::Int(i128::from(value)),
        MessagePackEvent::UInt(value) => Event::UInt(u128::from(value)),
        MessagePackEvent::Float32(value) => Event::Float32(value),
        MessagePackEvent::Float64(value) => Event::Float64(value),
        MessagePackEvent::String(value) => Event::String(value),
        MessagePackEvent::Binary(value) => Event::Bytes(value),
        MessagePackEvent::StartArray(length) => Event::StartArray(length),
        MessagePackEvent::EndArray => Event::EndArray,
        MessagePackEvent::StartMap(length) => Event::StartMap(length),
        MessagePackEvent::MapKey => Event::MapKey,
        MessagePackEvent::EndMap => Event::EndMap,
        MessagePackEvent::Ext(_) => return Err(error(MessagePackErrorKind::TypeMismatch, 0)),
    })
}

impl From<MessagePackValue> for serialization::Value {
    fn from(value: MessagePackValue) -> Self {
        match value {
            MessagePackValue::Nil => Self::Null,
            MessagePackValue::Bool(value) => Self::Bool(value),
            MessagePackValue::Int(value) => Self::Int(value),
            MessagePackValue::UInt(value) => Self::UInt(value),
            MessagePackValue::Float32(bits) => Self::Float32(bits),
            MessagePackValue::Float64(bits) => Self::Float64(bits),
            MessagePackValue::String(value) => Self::String(value),
            MessagePackValue::Binary(value) => Self::Bytes(value),
            MessagePackValue::Array(values) => {
                Self::Array(values.into_iter().map(Self::from).collect())
            }
            MessagePackValue::Map(entries) => Self::Map(
                entries
                    .into_iter()
                    .map(|entry| (Self::from(entry.key), Self::from(entry.value)))
                    .collect(),
            ),
            MessagePackValue::Ext(extension) => Self::Extension {
                type_code: extension.type_code,
                payload: extension.payload,
            },
        }
    }
}

impl TryFrom<serialization::Value> for MessagePackValue {
    type Error = MessagePackError;

    fn try_from(value: serialization::Value) -> Result<Self, Self::Error> {
        Ok(match value {
            serialization::Value::Null => Self::Nil,
            serialization::Value::Bool(value) => Self::Bool(value),
            serialization::Value::Int(value) => Self::Int(value),
            serialization::Value::UInt(value) => Self::UInt(value),
            serialization::Value::Float32(bits) => Self::Float32(bits),
            serialization::Value::Float64(bits) => Self::Float64(bits),
            serialization::Value::String(value) => Self::String(value),
            serialization::Value::Bytes(value) => Self::Binary(value),
            serialization::Value::Array(values) => Self::Array(
                values
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            serialization::Value::Map(entries) => Self::Map(
                entries
                    .into_iter()
                    .map(|(key, value)| {
                        Ok(MessagePackEntry {
                            key: Self::try_from(key)?,
                            value: Self::try_from(value)?,
                        })
                    })
                    .collect::<Result<Vec<_>, MessagePackError>>()?,
            ),
            serialization::Value::Extension { type_code, payload } => {
                Self::Ext(MessagePackExt { type_code, payload })
            }
            serialization::Value::Number(_) | serialization::Value::Object(_) => {
                return Err(error(MessagePackErrorKind::TypeMismatch, 0));
            }
        })
    }
}

impl Encoder<MessagePackCodec, MessagePackError> for MessagePackWriter {
    fn write_event(&mut self, event: Event) -> Result<(), MessagePackError> {
        self.write(serialization_event(&event)?)
    }
}

impl Decoder<MessagePackCodec, MessagePackError> for MessagePackReader<'_> {
    fn limits(&self) -> serialization::Limits {
        serialization::Limits {
            max_depth: self.options.limits.max_depth,
            max_events: self.options.limits.max_events,
            max_bytes: self.options.limits.max_document_bytes,
            max_container_items: self
                .options
                .limits
                .max_array_items
                .max(self.options.limits.max_map_pairs),
        }
    }

    fn peek_event(&mut self) -> Result<Option<Event>, MessagePackError> {
        let mut lookahead = self.clone();
        lookahead
            .next()?
            .map(messagepack_event_to_common)
            .transpose()
    }

    fn next(&mut self) -> Result<Option<Event>, MessagePackError> {
        MessagePackReader::next(self)?
            .map(messagepack_event_to_common)
            .transpose()
    }
}

/// Canonical typed MessagePack entry points for the static codec ABI.
pub fn encode_static<T: Encode<MessagePackCodec>>(
    value: &T,
    options: MessagePackEncodeOptions,
) -> Result<Vec<u8>, MessagePackError> {
    let mut writer = MessagePackWriter::new(options);
    value.encode(&mut writer)?;
    writer.finish()
}

pub fn decode_static<T: Decode<MessagePackCodec>>(
    input: &[u8],
    options: MessagePackDecodeOptions,
) -> Result<T, MessagePackError> {
    let mut reader = MessagePackReader::from_bytes(input, options)?;
    let value = T::decode(&mut reader)?;
    reader.finish()?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> MessagePackLimits {
        MessagePackLimits {
            max_document_bytes: 1024,
            max_depth: 8,
            max_array_items: 16,
            max_map_pairs: 16,
            max_string_bytes: 64,
            max_binary_bytes: 64,
            max_ext_bytes: 32,
            max_events: 128,
            max_output_bytes: 1024,
        }
    }

    #[test]
    fn dynamic_values_round_trip_all_wire_families() {
        let value = MessagePackValue::Map(vec![
            MessagePackEntry {
                key: MessagePackValue::String("s".into()),
                value: MessagePackValue::String("ok".into()),
            },
            MessagePackEntry {
                key: MessagePackValue::Int(-1),
                value: MessagePackValue::Array(vec![
                    MessagePackValue::Bool(true),
                    MessagePackValue::Float64((-0.0f64).to_bits()),
                ]),
            },
            MessagePackEntry {
                key: MessagePackValue::Binary(vec![0, 255]),
                value: MessagePackValue::Ext(MessagePackExt {
                    type_code: 2,
                    payload: vec![1, 2],
                }),
            },
        ]);
        let bytes = encode_value(
            &value,
            MessagePackEncodeOptions {
                limits: limits(),
                deterministic: false,
            },
        )
        .unwrap();
        assert_eq!(
            decode_value(
                &bytes,
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            )
            .unwrap(),
            value
        );
    }

    #[test]
    fn policies_nonminimal_duplicates_and_unknown_ext_are_explicit() {
        let duplicate = vec![0x82, 0x01, 0xc0, 0x01, 0xc3];
        assert!(matches!(
            decode_value(
                &duplicate,
                MessagePackDecodeOptions {
                    limits: limits(),
                    dynamic_map_duplicates: MessagePackDuplicatePolicy::Reject,
                    ..Default::default()
                }
            ),
            Err(MessagePackError {
                kind: MessagePackErrorKind::DuplicateKey,
                ..
            })
        ));
        let first = decode_value(
            &duplicate,
            MessagePackDecodeOptions {
                limits: limits(),
                dynamic_map_duplicates: MessagePackDuplicatePolicy::First,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            first,
            MessagePackValue::Map(vec![MessagePackEntry {
                key: MessagePackValue::UInt(1),
                value: MessagePackValue::Nil
            }])
        );
        assert!(matches!(
            decode_value(
                &[0xcc, 1],
                MessagePackDecodeOptions {
                    limits: limits(),
                    non_minimal: MessagePackNonMinimalPolicy::Reject,
                    ..Default::default()
                }
            ),
            Err(MessagePackError {
                kind: MessagePackErrorKind::NonMinimalEncoding,
                ..
            })
        ));
        assert!(matches!(
            decode_value(
                &[0xd4, 2, 0],
                MessagePackDecodeOptions {
                    limits: limits(),
                    unknown_extensions: MessagePackUnknownExtensionPolicy::Reject,
                    ..Default::default()
                }
            ),
            Err(MessagePackError {
                kind: MessagePackErrorKind::InvalidExtension,
                ..
            })
        ));
    }

    #[test]
    fn deterministic_maps_nan_and_collisions_are_stable() {
        let value = MessagePackValue::Map(vec![
            MessagePackEntry {
                key: MessagePackValue::String("z".into()),
                value: MessagePackValue::Float32(f32::NAN.to_bits()),
            },
            MessagePackEntry {
                key: MessagePackValue::String("a".into()),
                value: MessagePackValue::UInt(1),
            },
        ]);
        let bytes = encode_deterministic(&value).unwrap();
        let decoded = decode(&bytes).unwrap();
        assert!(
            matches!(decoded, MessagePackValue::Map(ref entries) if matches!(entries[0].key, MessagePackValue::String(ref key) if key == "a"))
        );
        let duplicate = MessagePackValue::Map(vec![
            MessagePackEntry {
                key: MessagePackValue::UInt(1),
                value: MessagePackValue::Nil,
            },
            MessagePackEntry {
                key: MessagePackValue::UInt(1),
                value: MessagePackValue::Nil,
            },
        ]);
        assert!(matches!(
            encode_deterministic(&duplicate),
            Err(MessagePackError {
                kind: MessagePackErrorKind::DeterministicKeyCollision,
                ..
            })
        ));
    }

    #[test]
    fn reader_emits_events_for_fragments_and_finishes_once() {
        let value = MessagePackValue::Array(vec![
            MessagePackValue::UInt(1),
            MessagePackValue::String("x".into()),
        ]);
        let bytes = encode(&value);
        let mut reader = MessagePackReader::from_chunks(
            bytes.chunks(1),
            MessagePackDecodeOptions {
                limits: limits(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reader.next().unwrap(),
            Some(MessagePackEvent::StartArray(Some(2)))
        );
        assert_eq!(reader.next().unwrap(), Some(MessagePackEvent::UInt(1)));
        assert_eq!(
            reader.next().unwrap(),
            Some(MessagePackEvent::String("x".into()))
        );
        assert_eq!(reader.next().unwrap(), Some(MessagePackEvent::EndArray));
        assert_eq!(reader.next().unwrap(), None);
        assert_eq!(reader.next().unwrap(), None);
        reader.finish().unwrap();
    }

    #[test]
    fn reader_and_writer_fail_terminally_on_bad_sequences() {
        let mut writer = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: limits(),
            deterministic: false,
        });
        assert!(writer.write(MessagePackEvent::EndArray).is_err());
        assert!(writer.write(MessagePackEvent::Nil).is_err());
        let mut writer = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: limits(),
            deterministic: false,
        });
        writer.write(MessagePackEvent::StartMap(Some(1))).unwrap();
        writer.write(MessagePackEvent::MapKey).unwrap();
        writer.write(MessagePackEvent::String("x".into())).unwrap();
        writer.write(MessagePackEvent::UInt(1)).unwrap();
        writer.write(MessagePackEvent::EndMap).unwrap();
        assert_eq!(
            decode(&writer.finish().unwrap()).unwrap(),
            MessagePackValue::Map(vec![MessagePackEntry {
                key: MessagePackValue::String("x".into()),
                value: MessagePackValue::UInt(1)
            }])
        );
    }

    #[test]
    fn timestamps_use_the_three_standard_payload_shapes() {
        for timestamp in [
            MessagePackTimestamp {
                seconds: 1,
                nanoseconds: 0,
            },
            MessagePackTimestamp {
                seconds: 1,
                nanoseconds: 2,
            },
            MessagePackTimestamp {
                seconds: -1,
                nanoseconds: 3,
            },
        ] {
            let ext = timestamp.to_ext().unwrap();
            assert_eq!(MessagePackTimestamp::from_ext(&ext).unwrap(), timestamp);
        }
        assert!(
            MessagePackTimestamp::from_ext(&MessagePackExt {
                type_code: 1,
                payload: vec![0]
            })
            .is_err()
        );
    }

    #[test]
    fn typed_events_use_the_common_static_traits() {
        let value: Vec<i64> = vec![1, -2, 3];
        let bytes = encode_typed(
            &value,
            MessagePackEncodeOptions {
                limits: limits(),
                deterministic: false,
            },
        )
        .unwrap();
        assert_eq!(
            decode_typed::<Vec<i64>>(
                &bytes,
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            )
            .unwrap(),
            value
        );
    }

    #[test]
    fn canonical_static_path_round_trips_scalars_and_arrays() {
        let options = MessagePackEncodeOptions::default();
        let bytes = encode_static(&vec![1_i32, 2, 3], options).unwrap();
        assert_eq!(bytes, vec![0x93, 1, 2, 3]);
        assert_eq!(
            decode_static::<Vec<i32>>(&bytes, MessagePackDecodeOptions::default()).unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            encode_static(&Option::<String>::None, options).unwrap(),
            vec![0xc0]
        );
        assert_eq!(
            decode_static::<Option<String>>(&[0xc0], MessagePackDecodeOptions::default()).unwrap(),
            None
        );
    }

    #[test]
    fn public_aliases_and_terminal_error_paths_are_exercised() {
        let options = MessagePackEncodeOptions {
            limits: limits(),
            deterministic: false,
        };
        let decode_options = MessagePackDecodeOptions {
            limits: limits(),
            ..Default::default()
        };
        let value = MessagePackValue::Array(vec![
            MessagePackValue::UInt(1),
            MessagePackValue::Map(vec![MessagePackEntry {
                key: MessagePackValue::String("k".into()),
                value: MessagePackValue::Bool(true),
            }]),
        ]);
        let encoded = encodeValue(&value, options).unwrap();
        assert_eq!(decodeValue(&encoded, decode_options).unwrap(), value);
        validate(&encoded, decode_options).unwrap();

        let deterministic = encode_deterministic_with_limits(&value, limits()).unwrap();
        assert_eq!(
            encodeDeterministic(&value, limits()).unwrap(),
            deterministic
        );

        let mut path = MessagePackPath::default();
        path.push(MessagePackPathSegment::MapValue);
        assert_eq!(path.segments(), &[MessagePackPathSegment::MapValue]);

        let mut writer = MessagePackWriter::to_writer(options);
        writer.write(MessagePackEvent::Nil).unwrap();
        assert_eq!(writer.finish().unwrap(), vec![0xc0]);
        let mut empty_writer = MessagePackWriter::new(options);
        assert!(empty_writer.write(MessagePackEvent::EndMap).is_err());

        let mut reader =
            MessagePackReader::from_chunks(vec![vec![0xc0_u8]], decode_options).unwrap();
        assert_eq!(reader.next().unwrap(), Some(MessagePackEvent::Nil));
        assert_eq!(reader.next().unwrap(), None);
        let mut malformed = MessagePackReader::from_bytes(&[0xc1], decode_options).unwrap();
        assert!(malformed.next().is_err());
        assert!(malformed.next().is_err());

        let timestamp = MessagePackTimestamp {
            seconds: 7,
            nanoseconds: 0,
        };
        let ext = timestamp.toExt().unwrap();
        assert_eq!(MessagePackTimestamp::fromExt(&ext).unwrap(), timestamp);

        let scalar = encode(&MessagePackValue::UInt(1));
        assert!(decode_typed::<Vec<i64>>(&scalar, decode_options).is_err());
        assert!(
            encode_typed(
                &vec![1_i64],
                MessagePackEncodeOptions {
                    limits: MessagePackLimits {
                        max_events: 0,
                        ..limits()
                    },
                    deterministic: false,
                }
            )
            .is_err()
        );

        let deterministic_array = MessagePackValue::Array(vec![MessagePackValue::UInt(1)]);
        assert!(encode_deterministic(&deterministic_array).is_ok());
    }

    #[test]
    fn parse_view_and_raw_preserve_wire_bytes_until_materialization() {
        // The non-minimal integer is deliberately retained by the view/raw
        // paths while clone_value observes the normal dynamic value.
        let input = [0xcc, 1_u8];
        let options = MessagePackDecodeOptions {
            limits: limits(),
            ..Default::default()
        };

        let view = parse_view(&input, options).unwrap();
        assert_eq!(view.bytes(), input);
        assert_eq!(view.clone_value().unwrap(), MessagePackValue::UInt(1));

        let raw_value = raw(&input, options).unwrap();
        assert_eq!(raw_value.as_bytes(), input);

        // The unchecked bridge is intentionally explicit and does not parse
        // or copy any semantic value beyond retaining the supplied bytes.
        let unchecked = raw_unchecked(&[0xc1]);
        assert_eq!(unchecked.as_bytes(), &[0xc1]);
    }

    #[test]
    fn limits_and_trailing_data_are_bounded() {
        let tiny = MessagePackLimits {
            max_document_bytes: 1,
            ..limits()
        };
        assert!(matches!(
            decode_value(
                &[0x92, 1],
                MessagePackDecodeOptions {
                    limits: tiny,
                    ..Default::default()
                }
            ),
            Err(MessagePackError {
                kind: MessagePackErrorKind::LimitExceeded,
                ..
            })
        ));
        assert!(matches!(
            decode(&[0xc0, 0xc0]),
            Err(MessagePackError {
                kind: MessagePackErrorKind::TrailingData,
                ..
            })
        ));
    }

    #[test]
    fn wire_widths_and_encoder_prefixes_cover_every_length_family() {
        let mut wide = limits();
        wide.max_document_bytes = 600_000;
        wide.max_string_bytes = 100_000;
        wide.max_binary_bytes = 100_000;
        wide.max_ext_bytes = 100_000;
        wide.max_array_items = 70_000;
        wide.max_map_pairs = 70_000;
        wide.max_events = 200_000;
        wide.max_output_bytes = 600_000;
        for value in [
            MessagePackValue::Int(127),
            MessagePackValue::Int(-33),
            MessagePackValue::Int(i8::MIN as i64),
            MessagePackValue::Int(i16::MIN as i64),
            MessagePackValue::Int(i32::MIN as i64),
            MessagePackValue::Int(i64::MIN),
            MessagePackValue::UInt(127),
            MessagePackValue::UInt(128),
            MessagePackValue::UInt(u16::MAX as u64 + 1),
            MessagePackValue::UInt(u32::MAX as u64 + 1),
            MessagePackValue::Float32(1.0f32.to_bits()),
            MessagePackValue::Float64(1.0f64.to_bits()),
        ] {
            let bytes = encode_value(
                &value,
                MessagePackEncodeOptions {
                    limits: wide,
                    deterministic: false,
                },
            )
            .unwrap();
            let decoded = decode_value(
                &bytes,
                MessagePackDecodeOptions {
                    limits: wide,
                    ..Default::default()
                },
            )
            .unwrap();
            if let MessagePackValue::Int(value) = value {
                if (0..=127).contains(&value) {
                    assert_eq!(decoded, MessagePackValue::UInt(value as u64));
                } else {
                    assert_eq!(decoded, MessagePackValue::Int(value));
                }
            } else {
                assert_eq!(decoded, value);
            }
        }
        for text in ["x".repeat(31), "x".repeat(32), "x".repeat(256)] {
            let bytes = encode_value(
                &MessagePackValue::String(text.clone()),
                MessagePackEncodeOptions {
                    limits: wide,
                    deterministic: false,
                },
            )
            .unwrap();
            assert_eq!(
                decode_value(
                    &bytes,
                    MessagePackDecodeOptions {
                        limits: wide,
                        ..Default::default()
                    }
                )
                .unwrap(),
                MessagePackValue::String(text)
            );
        }
        for bytes in [vec![1_u8; 32], vec![2_u8; 256]] {
            let encoded = encode_value(
                &MessagePackValue::Binary(bytes.clone()),
                MessagePackEncodeOptions {
                    limits: wide,
                    deterministic: false,
                },
            )
            .unwrap();
            assert_eq!(
                decode_value(
                    &encoded,
                    MessagePackDecodeOptions {
                        limits: wide,
                        ..Default::default()
                    }
                )
                .unwrap(),
                MessagePackValue::Binary(bytes)
            );
        }
        for length in [1_usize, 2, 4, 8, 16, 17, 256] {
            let value = MessagePackValue::Ext(MessagePackExt {
                type_code: 3,
                payload: vec![7; length],
            });
            let encoded = encode_value(
                &value,
                MessagePackEncodeOptions {
                    limits: wide,
                    deterministic: false,
                },
            )
            .unwrap();
            assert_eq!(
                decode_value(
                    &encoded,
                    MessagePackDecodeOptions {
                        limits: wide,
                        ..Default::default()
                    }
                )
                .unwrap(),
                value
            );
        }
        let array = MessagePackValue::Array(vec![MessagePackValue::Nil; 16]);
        let map = MessagePackValue::Map(
            (0..16)
                .map(|i| MessagePackEntry {
                    key: MessagePackValue::UInt(i),
                    value: MessagePackValue::Nil,
                })
                .collect(),
        );
        assert_eq!(
            decode_value(
                &encode(&array),
                MessagePackDecodeOptions {
                    limits: wide,
                    ..Default::default()
                }
            )
            .unwrap(),
            array
        );
        assert_eq!(
            decode_value(
                &encode(&map),
                MessagePackDecodeOptions {
                    limits: wide,
                    ..Default::default()
                }
            )
            .unwrap(),
            map
        );
        for value in [
            MessagePackValue::String("s".repeat(65_536)),
            MessagePackValue::Binary(vec![3; 65_536]),
            MessagePackValue::Array(vec![MessagePackValue::Nil; 65_536]),
            MessagePackValue::Map(
                (0..65_536)
                    .map(|i| MessagePackEntry {
                        key: MessagePackValue::UInt(i),
                        value: MessagePackValue::Nil,
                    })
                    .collect(),
            ),
        ] {
            let encoded = encode_value(
                &value,
                MessagePackEncodeOptions {
                    limits: wide,
                    deterministic: false,
                },
            )
            .unwrap();
            assert_eq!(
                decode_value(
                    &encoded,
                    MessagePackDecodeOptions {
                        limits: wide,
                        ..Default::default()
                    }
                )
                .unwrap(),
                value
            );
        }
        assert!(
            decode_value(
                &[0xc1],
                MessagePackDecodeOptions {
                    limits: wide,
                    ..Default::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn nonminimal_width_matrix_and_resource_boundaries_are_closed() {
        let mut options = MessagePackDecodeOptions {
            limits: limits(),
            non_minimal: MessagePackNonMinimalPolicy::Reject,
            ..Default::default()
        };
        for wire in [
            vec![0xcd, 0, 1],
            vec![0xce, 0, 0, 0, 1],
            vec![0xcf, 0, 0, 0, 0, 0, 0, 0, 1],
            vec![0xd1, 0, 1],
            vec![0xd2, 0, 0, 0, 0, 1],
            vec![0xda, 0, 1, b'x'],
            vec![0xdb, 0, 0, 0, 1, b'x'],
            vec![0xdc, 0, 0, 0],
            vec![0xdd, 0, 0, 0, 0, 0],
            vec![0xde, 0, 0, 0],
            vec![0xdf, 0, 0, 0, 0, 0],
        ] {
            assert!(matches!(
                decode_value(&wire, options),
                Err(MessagePackError {
                    kind: MessagePackErrorKind::NonMinimalEncoding,
                    ..
                })
            ));
        }
        options.non_minimal = MessagePackNonMinimalPolicy::Accept;
        assert_eq!(
            decode_value(&[0xcc, 1], options).unwrap(),
            MessagePackValue::UInt(1)
        );
        assert!(
            MessagePackReader::from_bytes(
                &[0xc0, 0xc0],
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            )
            .unwrap()
            .finish()
            .is_err()
        );
        assert!(
            MessagePackReader::from_bytes(
                &[],
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            )
            .unwrap()
            .finish()
            .is_err()
        );
        assert!(
            MessagePackReader::from_chunks(
                [vec![0xc0]],
                MessagePackDecodeOptions {
                    limits: MessagePackLimits {
                        max_document_bytes: 0,
                        ..limits()
                    },
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            MessagePackReader::from_reader(
                std::io::Cursor::new(vec![0xc0]),
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            )
            .unwrap()
            .finish()
            .is_ok()
        );
    }

    #[test]
    fn writer_events_cover_nested_keys_ownership_and_limits() {
        let mut writer = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: limits(),
            deterministic: false,
        });
        writer.write(MessagePackEvent::StartMap(None)).unwrap();
        writer.write(MessagePackEvent::MapKey).unwrap();
        writer.write(MessagePackEvent::StartArray(None)).unwrap();
        writer.write(MessagePackEvent::UInt(1)).unwrap();
        writer.write(MessagePackEvent::EndArray).unwrap();
        writer.write(MessagePackEvent::StartArray(None)).unwrap();
        writer.write(MessagePackEvent::String("v".into())).unwrap();
        writer.write(MessagePackEvent::EndArray).unwrap();
        writer.write(MessagePackEvent::EndMap).unwrap();
        let bytes = writer.finish().unwrap();
        assert!(matches!(decode(&bytes).unwrap(), MessagePackValue::Map(_)));
        assert!(writer.finish().is_err());
        let mut limited = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: MessagePackLimits {
                max_output_bytes: 1,
                ..limits()
            },
            deterministic: false,
        });
        assert!(
            limited
                .write(MessagePackEvent::String("too long".into()))
                .is_err()
        );
        assert!(limited.finish().is_err());
        let mut own = MessagePackReader::from_bytes(
            &[0xc4, 2, 1, 2],
            MessagePackDecodeOptions {
                limits: limits(),
                ..Default::default()
            },
        )
        .unwrap();
        let event = loop {
            match own.next().unwrap() {
                Some(MessagePackEvent::Binary(_)) => break own.next().unwrap(),
                Some(_) => continue,
                None => panic!("missing binary"),
            }
        };
        assert_eq!(event, None);
        assert_eq!(
            own.own(MessagePackEvent::Nil).unwrap(),
            MessagePackEvent::Nil
        );
        let mut deterministic = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: limits(),
            deterministic: true,
        });
        deterministic
            .write(MessagePackEvent::StartMap(None))
            .unwrap();
        deterministic.write(MessagePackEvent::MapKey).unwrap();
        deterministic
            .write(MessagePackEvent::String("z".into()))
            .unwrap();
        deterministic.write(MessagePackEvent::UInt(1)).unwrap();
        deterministic.write(MessagePackEvent::MapKey).unwrap();
        assert!(
            deterministic
                .write(MessagePackEvent::String("a".into()))
                .is_err()
        );
        assert!(deterministic.finish().is_err());
    }

    #[test]
    fn option_validation_and_timestamp_error_shapes_are_observable() {
        let bad = MessagePackLimits {
            max_depth: 0,
            ..limits()
        };
        assert!(
            MessagePackReader::from_bytes(
                &[0xc0],
                MessagePackDecodeOptions {
                    limits: bad,
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            encode_value(
                &MessagePackValue::Nil,
                MessagePackEncodeOptions {
                    limits: bad,
                    deterministic: false
                }
            )
            .is_err()
        );
        assert!(
            MessagePackTimestamp::from_ext(&MessagePackExt {
                type_code: -1,
                payload: vec![0, 0, 0, 0, 0]
            })
            .is_err()
        );
        assert!(
            MessagePackTimestamp::from_ext(&MessagePackExt {
                type_code: -1,
                payload: vec![0xff; 12]
            })
            .is_err()
        );
        assert!(
            MessagePackTimestamp {
                seconds: 0,
                nanoseconds: -1
            }
            .to_ext()
            .is_err()
        );
        let mut path = MessagePackPath::default();
        path.push(MessagePackPathSegment::ArrayIndex(1));
        path.push(MessagePackPathSegment::MapKey);
        assert_eq!(path.to_string(), "$[1][key]");
        assert_eq!(
            error(MessagePackErrorKind::InvalidTag, 4)
                .with_path(path)
                .to_string(),
            "MessagePack InvalidTag at byte 4"
        );
    }

    #[test]
    fn typed_and_host_boundaries_reject_unsupported_shapes_without_partial_values() {
        let ext = encode(&MessagePackValue::Ext(MessagePackExt {
            type_code: 2,
            payload: vec![1],
        }));
        assert!(matches!(
            decode_typed::<i64>(
                &ext,
                MessagePackDecodeOptions {
                    limits: limits(),
                    ..Default::default()
                }
            ),
            Err(MessagePackError {
                kind: MessagePackErrorKind::TypeMismatch,
                ..
            })
        ));
        let mut writer = MessagePackWriter::new(MessagePackEncodeOptions {
            limits: limits(),
            deterministic: false,
        });
        writer.write(MessagePackEvent::StartMap(Some(1))).unwrap();
        writer.write(MessagePackEvent::MapKey).unwrap();
        writer.write(MessagePackEvent::UInt(1)).unwrap();
        assert!(writer.finish().is_err());
        let mut reader = MessagePackReader::from_bytes(
            &[0xc4, 1, 7],
            MessagePackDecodeOptions {
                limits: limits(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            reader.own(MessagePackEvent::Nil).unwrap(),
            MessagePackEvent::Nil
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(MessagePackEvent::Binary(vec![7]))
        );
        assert_eq!(reader.next().unwrap(), None);
    }
}
