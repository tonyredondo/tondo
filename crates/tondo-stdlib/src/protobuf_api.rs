//! Public, bounded Protocol Buffers owner.
//!
//! The small wire kernel in `protobuf.rs` remains available to the hosted
//! bridge.  This module owns the schema-bound surface: all decoding is driven
//! by an explicit frame stack, unknown fields retain their exact wire bytes,
//! and the typed adapter consumes the common static serialization events.

use std::borrow::Cow;
use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::io::{Read, Write};
use std::marker::PhantomData;

use crate::serialization::{self, Deserialize, Event, Serialize};

const MAX_FIELD_NUMBER: u32 = 536_870_911;
const RESERVED_FIELD_START: u32 = 19_000;
const RESERVED_FIELD_END: u32 = 19_999;
const MAX_LENGTH: u64 = 2_147_483_647;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum ProtoWireType {
    Varint = 0,
    Fixed64 = 1,
    LengthDelimited = 2,
    StartGroup = 3,
    EndGroup = 4,
    Fixed32 = 5,
}

impl ProtoWireType {
    fn from_byte(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Varint,
            1 => Self::Fixed64,
            2 => Self::LengthDelimited,
            3 => Self::StartGroup,
            4 => Self::EndGroup,
            5 => Self::Fixed32,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtoPathSegment {
    Message(String),
    FieldNumber(u32),
    RepeatedIndex(usize),
    MapKey(String),
    MapValue,
    OneofCase(String),
    UnknownField(u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtoPath(Vec<ProtoPathSegment>);

impl ProtoPath {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn segments(&self) -> &[ProtoPathSegment] {
        &self.0
    }

    pub fn push(&mut self, segment: ProtoPathSegment) {
        self.0.push(segment);
    }
}

impl fmt::Display for ProtoPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        for segment in &self.0 {
            match segment {
                ProtoPathSegment::Message(name) => write!(formatter, ".{name}")?,
                ProtoPathSegment::FieldNumber(number) => write!(formatter, ".field[{number}]")?,
                ProtoPathSegment::RepeatedIndex(index) => write!(formatter, "[{index}]")?,
                ProtoPathSegment::MapKey(key) => write!(formatter, "[key={key:?}]")?,
                ProtoPathSegment::MapValue => formatter.write_str("[value]")?,
                ProtoPathSegment::OneofCase(name) => write!(formatter, ".oneof[{name}]")?,
                ProtoPathSegment::UnknownField(number) => write!(formatter, ".unknown[{number}]")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoLimits {
    pub max_schema_bytes: usize,
    pub max_imports: usize,
    pub max_generated_types: usize,
    pub max_generated_bytes: usize,
    pub max_message_bytes: usize,
    pub max_depth: usize,
    pub max_fields: usize,
    pub max_repeated_items: usize,
    pub max_map_entries: usize,
    pub max_string_bytes: usize,
    pub max_bytes_field_bytes: usize,
    pub max_packed_bytes: usize,
    pub max_unknown_bytes: usize,
    pub max_varint_bytes: usize,
    pub max_events: usize,
    pub max_output_bytes: usize,
}

impl Default for ProtoLimits {
    fn default() -> Self {
        Self {
            max_schema_bytes: 16 * 1024 * 1024,
            max_imports: 256,
            max_generated_types: 100_000,
            max_generated_bytes: 256 * 1024 * 1024,
            max_message_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_fields: 1_000_000,
            max_repeated_items: 1_000_000,
            max_map_entries: 1_000_000,
            max_string_bytes: 64 * 1024 * 1024,
            max_bytes_field_bytes: 64 * 1024 * 1024,
            max_packed_bytes: 64 * 1024 * 1024,
            max_unknown_bytes: 64 * 1024 * 1024,
            max_varint_bytes: 10,
            max_events: 1_000_000,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

impl ProtoLimits {
    fn valid(self) -> bool {
        self.max_schema_bytes > 0
            && self.max_message_bytes > 0
            && self.max_depth > 0
            && self.max_fields > 0
            && self.max_repeated_items > 0
            && self.max_map_entries > 0
            && self.max_string_bytes > 0
            && self.max_bytes_field_bytes > 0
            && self.max_packed_bytes > 0
            && self.max_unknown_bytes > 0
            && (1..=10).contains(&self.max_varint_bytes)
            && self.max_events > 0
            && self.max_output_bytes > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoWireTypePolicy {
    PreserveUnknown,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoUnknownPolicy {
    Preserve,
    Discard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoDecodeOptions {
    pub limits: ProtoLimits,
    pub wire_type: ProtoWireTypePolicy,
    pub unknown_fields: ProtoUnknownPolicy,
    pub reject_non_minimal_varints: bool,
}

impl Default for ProtoDecodeOptions {
    fn default() -> Self {
        Self {
            limits: ProtoLimits::default(),
            wire_type: ProtoWireTypePolicy::PreserveUnknown,
            unknown_fields: ProtoUnknownPolicy::Preserve,
            reject_non_minimal_varints: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProtoEncodeOptions {
    pub limits: ProtoLimits,
    pub deterministic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoErrorKind {
    UnexpectedEof,
    InvalidTag,
    InvalidWireType,
    InvalidVarint,
    InvalidLength,
    InvalidUtf8,
    TypeMismatch,
    InvalidPacked,
    NumberRange,
    InvalidFieldNumber,
    InvalidGroup,
    LimitExceeded,
    IoError,
    TrailingData,
    SchemaMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoError {
    pub kind: ProtoErrorKind,
    pub offset: Option<usize>,
    pub path: ProtoPath,
}

impl ProtoError {
    fn new(kind: ProtoErrorKind, offset: usize) -> Self {
        Self {
            kind,
            offset: Some(offset),
            path: ProtoPath::root(),
        }
    }

    fn terminal(kind: ProtoErrorKind) -> Self {
        Self {
            kind,
            offset: None,
            path: ProtoPath::root(),
        }
    }
}

impl fmt::Display for ProtoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Protobuf {:?} at {}", self.kind, self.path)?;
        if let Some(offset) = self.offset {
            write!(formatter, " (byte {offset})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ProtoError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtoBuildErrorKind {
    ProtoSyntaxUnsupported,
    ProtoImportNotDeclared,
    ProtoNameCollision,
    ProtoFieldNumberConflict,
    ProtoReservedReuse,
    ProtoSchemaDrift,
    ProtoWireIncompatible,
    ProtoGeneratorOutputCollision,
    ProtoGenerationLimit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoBuildError {
    pub kind: ProtoBuildErrorKind,
    pub schema: String,
    pub path: String,
}

impl fmt::Display for ProtoBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Protobuf build {:?}: {}", self.kind, self.schema)
    }
}

impl std::error::Error for ProtoBuildError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownField {
    pub number: u32,
    pub wire_type: ProtoWireType,
    pub tag_bytes: Vec<u8>,
    /// Bytes after the tag, including the length prefix for LEN and the
    /// matching end-group tag for a preserved group.
    pub payload_bytes: Vec<u8>,
}

impl UnknownField {
    pub fn raw_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.tag_bytes.len() + self.payload_bytes.len());
        bytes.extend_from_slice(&self.tag_bytes);
        bytes.extend_from_slice(&self.payload_bytes);
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnknownFields(Vec<UnknownField>);

impl UnknownFields {
    pub fn push(&mut self, field: UnknownField) {
        self.0.push(field);
    }

    pub fn count(&self) -> usize {
        self.0.len()
    }

    pub fn discard(&mut self) {
        self.0.clear();
    }

    #[allow(non_snake_case)]
    pub fn discardUnknown(&mut self) {
        self.discard();
    }
}

impl std::ops::Deref for UnknownFields {
    type Target = [UnknownField];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for UnknownFields {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProtoEvent {
    StartMessage(String),
    EndMessage,
    Field(u32, ProtoWireType),
    Varint(u64),
    Fixed32(u32),
    Fixed64(u64),
    StartLengthDelimited(u32),
    Bytes(Vec<u8>),
    EndLengthDelimited,
    StartPacked(u32),
    EndPacked,
    Unknown(UnknownField),
}

/// A dynamic wire value useful to bridges and tests.  Generated Tondo records
/// use the same operations but never need this type at runtime.
#[derive(Debug, Clone, PartialEq)]
pub enum ProtoValue {
    Varint(u64),
    Fixed32(u32),
    Fixed64(u64),
    Bytes(Vec<u8>),
    Message(Vec<ProtoField>),
    Unknown(UnknownField),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProtoField {
    pub number: u32,
    pub value: ProtoValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtoDescriptor<T> {
    pub name: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T> ProtoDescriptor<T> {
    pub fn name(&self) -> &'static str {
        self.name
    }
}

pub fn descriptor<T>() -> ProtoDescriptor<T> {
    ProtoDescriptor {
        name: std::any::type_name::<T>(),
        _marker: PhantomData,
    }
}

#[derive(Debug, Clone)]
struct ParsedField {
    number: u32,
    wire_type: ProtoWireType,
    tag_bytes: Vec<u8>,
    payload: Vec<u8>,
    raw_after_tag: Vec<u8>,
}

fn checked_limits(limits: ProtoLimits) -> Result<(), ProtoError> {
    if limits.valid() {
        Ok(())
    } else {
        Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0))
    }
}

fn field_number(key: u64) -> Result<u32, ProtoErrorKind> {
    let number = u32::try_from(key >> 3).map_err(|_| ProtoErrorKind::InvalidFieldNumber)?;
    if number == 0 || number > MAX_FIELD_NUMBER {
        return Err(ProtoErrorKind::InvalidFieldNumber);
    }
    if (RESERVED_FIELD_START..=RESERVED_FIELD_END).contains(&number) {
        return Err(ProtoErrorKind::InvalidFieldNumber);
    }
    Ok(number)
}

fn minimal_varint_len(mut value: u64) -> usize {
    let mut length = 1;
    while value >= 0x80 {
        value >>= 7;
        length += 1;
    }
    length
}

fn read_varint(
    input: &[u8],
    offset: &mut usize,
    options: ProtoDecodeOptions,
) -> Result<(u64, Vec<u8>), ProtoError> {
    let start = *offset;
    let mut value = 0u64;
    for index in 0..options.limits.max_varint_bytes {
        let byte = *input
            .get(*offset)
            .ok_or_else(|| ProtoError::new(ProtoErrorKind::UnexpectedEof, *offset))?;
        *offset += 1;
        if index == 9 && byte > 1 {
            return Err(ProtoError::new(ProtoErrorKind::InvalidVarint, start));
        }
        value |= u64::from(byte & 0x7f) << (index * 7);
        if byte & 0x80 == 0 {
            let encoded = &input[start..*offset];
            if options.reject_non_minimal_varints && encoded.len() != minimal_varint_len(value) {
                return Err(ProtoError::new(ProtoErrorKind::InvalidVarint, start));
            }
            return Ok((value, encoded.to_vec()));
        }
    }
    Err(ProtoError::new(ProtoErrorKind::InvalidVarint, start))
}

fn take(input: &[u8], offset: &mut usize, length: usize) -> Result<Vec<u8>, ProtoError> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| ProtoError::new(ProtoErrorKind::InvalidLength, *offset))?;
    let bytes = input
        .get(*offset..end)
        .ok_or_else(|| ProtoError::new(ProtoErrorKind::UnexpectedEof, *offset))?
        .to_vec();
    *offset = end;
    Ok(bytes)
}

fn consume_group(
    input: &[u8],
    offset: &mut usize,
    root: u32,
    options: ProtoDecodeOptions,
) -> Result<(), ProtoError> {
    let mut stack = vec![root];
    while let Some(expected) = stack.last().copied() {
        if stack.len() > options.limits.max_depth {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, *offset));
        }
        let (key, _) = read_varint(input, offset, options)?;
        let number = field_number(key).map_err(|kind| ProtoError::new(kind, *offset))?;
        let wire = ProtoWireType::from_byte((key & 7) as u8)
            .ok_or_else(|| ProtoError::new(ProtoErrorKind::InvalidWireType, *offset))?;
        match wire {
            ProtoWireType::StartGroup => stack.push(number),
            ProtoWireType::EndGroup => {
                if number != expected {
                    return Err(ProtoError::new(ProtoErrorKind::InvalidGroup, *offset));
                }
                stack.pop();
            }
            ProtoWireType::Varint => {
                read_varint(input, offset, options)?;
            }
            ProtoWireType::Fixed64 => {
                take(input, offset, 8)?;
            }
            ProtoWireType::Fixed32 => {
                take(input, offset, 4)?;
            }
            ProtoWireType::LengthDelimited => {
                let (length, _) = read_varint(input, offset, options)?;
                if length > MAX_LENGTH || length > options.limits.max_message_bytes as u64 {
                    return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, *offset));
                }
                take(input, offset, length as usize)?;
            }
        }
    }
    Ok(())
}

fn parse_field(
    input: &[u8],
    offset: &mut usize,
    options: ProtoDecodeOptions,
) -> Result<ParsedField, ProtoError> {
    let start = *offset;
    let (key, tag_bytes) = read_varint(input, offset, options)?;
    if key < 8 {
        return Err(ProtoError::new(ProtoErrorKind::InvalidTag, start));
    }
    let number = field_number(key).map_err(|kind| ProtoError::new(kind, start))?;
    let wire_type = ProtoWireType::from_byte((key & 7) as u8)
        .ok_or_else(|| ProtoError::new(ProtoErrorKind::InvalidWireType, start))?;
    if wire_type == ProtoWireType::EndGroup {
        return Err(ProtoError::new(ProtoErrorKind::InvalidGroup, start));
    }
    let payload_start = *offset;
    let payload = match wire_type {
        ProtoWireType::Varint => read_varint(input, offset, options)?.1,
        ProtoWireType::Fixed64 => take(input, offset, 8)?,
        ProtoWireType::LengthDelimited => {
            let (length, _) = read_varint(input, offset, options)?;
            if length > MAX_LENGTH || length > options.limits.max_message_bytes as u64 {
                return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, *offset));
            }
            let bytes = take(input, offset, length as usize)?;
            if bytes.len() > options.limits.max_bytes_field_bytes {
                return Err(ProtoError::new(
                    ProtoErrorKind::LimitExceeded,
                    payload_start,
                ));
            }
            bytes
        }
        ProtoWireType::StartGroup => {
            consume_group(input, offset, number, options)?;
            input[payload_start..*offset].to_vec()
        }
        ProtoWireType::EndGroup => unreachable!(),
        ProtoWireType::Fixed32 => take(input, offset, 4)?,
    };
    Ok(ParsedField {
        number,
        wire_type,
        tag_bytes,
        payload,
        raw_after_tag: input[payload_start..*offset].to_vec(),
    })
}

fn parse_fields(input: &[u8], options: ProtoDecodeOptions) -> Result<Vec<ParsedField>, ProtoError> {
    checked_limits(options.limits)?;
    if input.len() > options.limits.max_message_bytes {
        return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
    }
    let mut fields = Vec::new();
    let mut offset = 0;
    while offset < input.len() {
        if fields.len() >= options.limits.max_fields {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, offset));
        }
        fields.push(parse_field(input, &mut offset, options)?);
    }
    Ok(fields)
}

fn unknown_from(field: &ParsedField) -> UnknownField {
    UnknownField {
        number: field.number,
        wire_type: field.wire_type,
        tag_bytes: field.tag_bytes.clone(),
        payload_bytes: field.raw_after_tag.clone(),
    }
}

fn encode_key(
    number: u32,
    wire_type: ProtoWireType,
    output: &mut Vec<u8>,
) -> Result<(), ProtoError> {
    if number == 0
        || number > MAX_FIELD_NUMBER
        || (RESERVED_FIELD_START..=RESERVED_FIELD_END).contains(&number)
    {
        return Err(ProtoError::new(
            ProtoErrorKind::InvalidFieldNumber,
            output.len(),
        ));
    }
    encode_varint((u64::from(number) << 3) | wire_type as u64, output);
    Ok(())
}

fn encode_varint(mut value: u64, output: &mut Vec<u8>) {
    while value >= 0x80 {
        output.push(value as u8 | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
}

fn encode_proto_value(
    field: &ProtoField,
    output: &mut Vec<u8>,
    options: ProtoEncodeOptions,
) -> Result<(), ProtoError> {
    match &field.value {
        ProtoValue::Varint(value) => {
            encode_key(field.number, ProtoWireType::Varint, output)?;
            encode_varint(*value, output);
        }
        ProtoValue::Fixed32(value) => {
            encode_key(field.number, ProtoWireType::Fixed32, output)?;
            output.extend_from_slice(&value.to_le_bytes());
        }
        ProtoValue::Fixed64(value) => {
            encode_key(field.number, ProtoWireType::Fixed64, output)?;
            output.extend_from_slice(&value.to_le_bytes());
        }
        ProtoValue::Bytes(value) => {
            if value.len() > options.limits.max_bytes_field_bytes {
                return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, output.len()));
            }
            encode_key(field.number, ProtoWireType::LengthDelimited, output)?;
            encode_varint(value.len() as u64, output);
            output.extend_from_slice(value);
        }
        ProtoValue::Message(fields) => {
            let nested = encode_message(fields, options)?;
            encode_key(field.number, ProtoWireType::LengthDelimited, output)?;
            encode_varint(nested.len() as u64, output);
            output.extend_from_slice(&nested);
        }
        ProtoValue::Unknown(unknown) => output.extend_from_slice(&unknown.raw_bytes()),
    }
    if output.len() > options.limits.max_output_bytes {
        return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, output.len()));
    }
    Ok(())
}

pub fn encode_message(
    fields: &[ProtoField],
    options: ProtoEncodeOptions,
) -> Result<Vec<u8>, ProtoError> {
    checked_limits(options.limits)?;
    let mut ordered: Vec<&ProtoField> = fields.iter().collect();
    if options.deterministic {
        ordered.sort_by_key(|field| field.number);
    }
    let mut output = Vec::new();
    for field in ordered {
        encode_proto_value(field, &mut output, options)?;
    }
    Ok(output)
}

pub fn decode_message(
    input: &[u8],
    options: ProtoDecodeOptions,
) -> Result<Vec<ProtoField>, ProtoError> {
    parse_fields(input, options)?
        .into_iter()
        .map(|field| {
            let value = match field.wire_type {
                ProtoWireType::Varint => {
                    let mut offset = 0;
                    let (value, _) = read_varint(&field.payload, &mut offset, options)?;
                    ProtoValue::Varint(value)
                }
                ProtoWireType::Fixed32 => ProtoValue::Fixed32(u32::from_le_bytes(
                    field
                        .payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
                )),
                ProtoWireType::Fixed64 => ProtoValue::Fixed64(u64::from_le_bytes(
                    field
                        .payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
                )),
                ProtoWireType::LengthDelimited => ProtoValue::Bytes(field.payload),
                ProtoWireType::StartGroup => ProtoValue::Unknown(unknown_from(&field)),
                ProtoWireType::EndGroup => {
                    return Err(ProtoError::new(ProtoErrorKind::InvalidGroup, 0));
                }
            };
            Ok(ProtoField {
                number: field.number,
                value,
            })
        })
        .collect()
}

fn parsed_to_events(
    fields: &[ParsedField],
    options: ProtoDecodeOptions,
) -> Result<Vec<ProtoEvent>, ProtoError> {
    let mut events = Vec::new();
    events.push(ProtoEvent::StartMessage("root".to_owned()));
    let mut unknown_bytes = 0usize;
    for field in fields {
        if events.len() >= options.limits.max_events {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, events.len()));
        }
        match field.wire_type {
            ProtoWireType::Varint => {
                let mut cursor = 0;
                let (value, _) = read_varint(&field.payload, &mut cursor, options)?;
                events.push(ProtoEvent::Field(field.number, field.wire_type));
                events.push(ProtoEvent::Varint(value));
            }
            ProtoWireType::Fixed32 => {
                let value = u32::from_le_bytes(
                    field
                        .payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
                );
                events.push(ProtoEvent::Field(field.number, field.wire_type));
                events.push(ProtoEvent::Fixed32(value));
            }
            ProtoWireType::Fixed64 => {
                let value = u64::from_le_bytes(
                    field
                        .payload
                        .as_slice()
                        .try_into()
                        .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
                );
                events.push(ProtoEvent::Field(field.number, field.wire_type));
                events.push(ProtoEvent::Fixed64(value));
            }
            ProtoWireType::LengthDelimited => {
                if field.payload.len() > options.limits.max_packed_bytes {
                    return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
                }
                events.push(ProtoEvent::StartLengthDelimited(field.number));
                events.push(ProtoEvent::Bytes(field.payload.clone()));
                events.push(ProtoEvent::EndLengthDelimited);
            }
            ProtoWireType::StartGroup => {
                unknown_bytes = unknown_bytes.saturating_add(field.raw_after_tag.len());
                if unknown_bytes > options.limits.max_unknown_bytes {
                    return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
                }
                if options.unknown_fields == ProtoUnknownPolicy::Preserve {
                    events.push(ProtoEvent::Unknown(unknown_from(field)));
                }
            }
            ProtoWireType::EndGroup => {
                return Err(ProtoError::new(ProtoErrorKind::InvalidGroup, 0));
            }
        }
    }
    events.push(ProtoEvent::EndMessage);
    Ok(events)
}

pub struct ProtoReader<'a, T = ()> {
    input: Cow<'a, [u8]>,
    options: ProtoDecodeOptions,
    events: VecDeque<ProtoEvent>,
    loaded: bool,
    emitted_none: bool,
    terminal: Option<ProtoError>,
    _marker: PhantomData<fn() -> T>,
}

impl<'a, T> ProtoReader<'a, T> {
    pub fn from_bytes(input: &'a [u8], options: ProtoDecodeOptions) -> Result<Self, ProtoError> {
        checked_limits(options.limits)?;
        if input.len() > options.limits.max_message_bytes {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
        }
        Ok(Self {
            input: Cow::Borrowed(input),
            options,
            events: VecDeque::new(),
            loaded: false,
            emitted_none: false,
            terminal: None,
            _marker: PhantomData,
        })
    }

    #[allow(non_snake_case)]
    pub fn fromBytes(input: &'a [u8], options: ProtoDecodeOptions) -> Result<Self, ProtoError> {
        Self::from_bytes(input, options)
    }

    pub fn from_chunks<I, B>(
        chunks: I,
        options: ProtoDecodeOptions,
    ) -> Result<ProtoReader<'static, T>, ProtoError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        checked_limits(options.limits)?;
        let mut bytes = Vec::new();
        for chunk in chunks {
            let chunk = chunk.as_ref();
            let end = bytes
                .len()
                .checked_add(chunk.len())
                .ok_or_else(|| ProtoError::new(ProtoErrorKind::LimitExceeded, bytes.len()))?;
            if end > options.limits.max_message_bytes {
                return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, bytes.len()));
            }
            bytes.extend_from_slice(chunk);
        }
        Ok(ProtoReader {
            input: Cow::Owned(bytes),
            options,
            events: VecDeque::new(),
            loaded: false,
            emitted_none: false,
            terminal: None,
            _marker: PhantomData,
        })
    }

    #[allow(non_snake_case)]
    pub fn fromChunks<I, B>(
        chunks: I,
        options: ProtoDecodeOptions,
    ) -> Result<ProtoReader<'static, T>, ProtoError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        Self::from_chunks(chunks, options)
    }

    pub fn from_reader<R: Read>(
        mut input: R,
        options: ProtoDecodeOptions,
    ) -> Result<ProtoReader<'static, T>, ProtoError> {
        checked_limits(options.limits)?;
        let mut bytes = Vec::new();
        input
            .by_ref()
            .take(options.limits.max_message_bytes.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| ProtoError::new(ProtoErrorKind::IoError, 0))?;
        if bytes.len() > options.limits.max_message_bytes {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
        }
        Ok(ProtoReader {
            input: Cow::Owned(bytes),
            options,
            events: VecDeque::new(),
            loaded: false,
            emitted_none: false,
            terminal: None,
            _marker: PhantomData,
        })
    }

    #[allow(non_snake_case)]
    pub fn fromReader<R: Read>(
        input: R,
        options: ProtoDecodeOptions,
    ) -> Result<ProtoReader<'static, T>, ProtoError> {
        Self::from_reader(input, options)
    }

    fn fail<U>(&mut self, error: ProtoError) -> Result<U, ProtoError> {
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn load(&mut self) -> Result<(), ProtoError> {
        let fields = parse_fields(&self.input, self.options)?;
        self.events = parsed_to_events(&fields, self.options)?
            .into_iter()
            .collect();
        self.loaded = true;
        Ok(())
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<ProtoEvent>, ProtoError> {
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
        self.emitted_none = true;
        Ok(None)
    }

    pub fn own(&mut self, event: ProtoEvent) -> Result<ProtoEvent, ProtoError> {
        if self.terminal.is_some() {
            return self.fail(ProtoError::terminal(ProtoErrorKind::TypeMismatch));
        }
        Ok(event)
    }

    pub fn finish(&mut self) -> Result<(), ProtoError> {
        while self.next()?.is_some() {}
        Ok(())
    }
}

#[derive(Debug)]
struct MessageFrame {
    bytes: Vec<u8>,
    pending_field: Option<(u32, ProtoWireType)>,
}

#[derive(Debug)]
struct LengthFrame {
    number: u32,
    bytes: Vec<u8>,
}

#[derive(Debug)]
enum WriterFrame {
    Message(MessageFrame),
    Length(LengthFrame),
    Packed(LengthFrame),
}

pub struct ProtoWriter {
    options: ProtoEncodeOptions,
    stack: Vec<WriterFrame>,
    output: Option<Vec<u8>>,
    sink: Option<Box<dyn Write>>,
    terminal: Option<ProtoError>,
    finished: bool,
}

impl ProtoWriter {
    pub fn new(options: ProtoEncodeOptions) -> Self {
        Self {
            options,
            stack: Vec::new(),
            output: None,
            sink: None,
            terminal: None,
            finished: false,
        }
    }

    pub fn to_writer<W: Write + 'static>(output: W, options: ProtoEncodeOptions) -> Self {
        let mut writer = Self::new(options);
        writer.sink = Some(Box::new(output));
        writer
    }

    #[allow(non_snake_case)]
    pub fn toWriter<W: Write + 'static>(output: W, options: ProtoEncodeOptions) -> Self {
        Self::to_writer(output, options)
    }

    fn fail<U>(&mut self, error: ProtoError) -> Result<U, ProtoError> {
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn append_to_message(&mut self, bytes: &[u8]) -> Result<(), ProtoError> {
        match self.stack.last_mut() {
            Some(WriterFrame::Message(frame)) => {
                if frame.bytes.len().saturating_add(bytes.len())
                    > self.options.limits.max_output_bytes
                {
                    return Err(ProtoError::new(
                        ProtoErrorKind::LimitExceeded,
                        frame.bytes.len(),
                    ));
                }
                frame.bytes.extend_from_slice(bytes);
                Ok(())
            }
            _ => Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0)),
        }
    }

    fn finish_scalar(&mut self, wire: ProtoWireType, payload: &[u8]) -> Result<(), ProtoError> {
        let Some(WriterFrame::Message(frame)) = self.stack.last_mut() else {
            return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
        };
        let Some((number, expected)) = frame.pending_field.take() else {
            return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
        };
        if expected != wire {
            return Err(ProtoError::new(
                ProtoErrorKind::InvalidWireType,
                frame.bytes.len(),
            ));
        }
        let mut encoded = Vec::new();
        encode_key(number, wire, &mut encoded)?;
        encoded.extend_from_slice(payload);
        if frame.bytes.len().saturating_add(encoded.len()) > self.options.limits.max_output_bytes {
            return Err(ProtoError::new(
                ProtoErrorKind::LimitExceeded,
                frame.bytes.len(),
            ));
        }
        frame.bytes.extend_from_slice(&encoded);
        Ok(())
    }

    fn finish_length(&mut self, number: u32, bytes: Vec<u8>) -> Result<(), ProtoError> {
        if bytes.len() > self.options.limits.max_bytes_field_bytes {
            return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
        }
        let mut encoded = Vec::new();
        encode_key(number, ProtoWireType::LengthDelimited, &mut encoded)?;
        encode_varint(bytes.len() as u64, &mut encoded);
        encoded.extend_from_slice(&bytes);
        self.append_to_message(&encoded)
    }

    pub fn write(&mut self, event: ProtoEvent) -> Result<(), ProtoError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return self.fail(ProtoError::terminal(ProtoErrorKind::TrailingData));
        }
        if let Err(error) =
            checked_limits(self.options.limits).and_then(|()| self.write_inner(event))
        {
            return self.fail(error);
        }
        Ok(())
    }

    fn write_inner(&mut self, event: ProtoEvent) -> Result<(), ProtoError> {
        match event {
            ProtoEvent::StartMessage(_) => {
                if self.stack.len() >= self.options.limits.max_depth {
                    return Err(ProtoError::new(
                        ProtoErrorKind::LimitExceeded,
                        self.stack.len(),
                    ));
                }
                if !self.stack.is_empty()
                    && !matches!(self.stack.last(), Some(WriterFrame::Length(_)))
                {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                }
                self.stack.push(WriterFrame::Message(MessageFrame {
                    bytes: Vec::new(),
                    pending_field: None,
                }));
                Ok(())
            }
            ProtoEvent::EndMessage => {
                let Some(WriterFrame::Message(frame)) = self.stack.pop() else {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                };
                if frame.pending_field.is_some() {
                    return Err(ProtoError::new(
                        ProtoErrorKind::TypeMismatch,
                        frame.bytes.len(),
                    ));
                }
                if let Some(WriterFrame::Length(length)) = self.stack.last_mut() {
                    length.bytes.extend_from_slice(&frame.bytes);
                    Ok(())
                } else if self.stack.is_empty() && self.output.is_none() {
                    self.output = Some(frame.bytes);
                    Ok(())
                } else {
                    Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0))
                }
            }
            ProtoEvent::Field(number, wire_type) => {
                let Some(WriterFrame::Message(frame)) = self.stack.last_mut() else {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                };
                if frame.pending_field.is_some() {
                    return Err(ProtoError::new(
                        ProtoErrorKind::TypeMismatch,
                        frame.bytes.len(),
                    ));
                }
                if wire_type == ProtoWireType::LengthDelimited
                    || wire_type == ProtoWireType::StartGroup
                    || wire_type == ProtoWireType::EndGroup
                {
                    return Err(ProtoError::new(
                        ProtoErrorKind::TypeMismatch,
                        frame.bytes.len(),
                    ));
                }
                frame.pending_field = Some((number, wire_type));
                Ok(())
            }
            ProtoEvent::Varint(value) => {
                let mut bytes = Vec::new();
                encode_varint(value, &mut bytes);
                self.finish_scalar(ProtoWireType::Varint, &bytes)
            }
            ProtoEvent::Fixed32(value) => {
                self.finish_scalar(ProtoWireType::Fixed32, &value.to_le_bytes())
            }
            ProtoEvent::Fixed64(value) => {
                self.finish_scalar(ProtoWireType::Fixed64, &value.to_le_bytes())
            }
            ProtoEvent::StartLengthDelimited(number) => {
                if !matches!(self.stack.last(), Some(WriterFrame::Message(_))) {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                }
                self.stack.push(WriterFrame::Length(LengthFrame {
                    number,
                    bytes: Vec::new(),
                }));
                Ok(())
            }
            ProtoEvent::Bytes(bytes) => match self.stack.last_mut() {
                Some(WriterFrame::Length(frame)) | Some(WriterFrame::Packed(frame)) => {
                    if frame.bytes.len().saturating_add(bytes.len())
                        > self.options.limits.max_bytes_field_bytes
                    {
                        return Err(ProtoError::new(
                            ProtoErrorKind::LimitExceeded,
                            frame.bytes.len(),
                        ));
                    }
                    frame.bytes.extend_from_slice(&bytes);
                    Ok(())
                }
                _ => Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0)),
            },
            ProtoEvent::EndLengthDelimited => {
                let Some(WriterFrame::Length(frame)) = self.stack.pop() else {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                };
                self.finish_length(frame.number, frame.bytes)
            }
            ProtoEvent::StartPacked(number) => {
                if !matches!(self.stack.last(), Some(WriterFrame::Message(_))) {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                }
                self.stack.push(WriterFrame::Packed(LengthFrame {
                    number,
                    bytes: Vec::new(),
                }));
                Ok(())
            }
            ProtoEvent::EndPacked => {
                let Some(WriterFrame::Packed(frame)) = self.stack.pop() else {
                    return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0));
                };
                self.finish_length(frame.number, frame.bytes)
            }
            ProtoEvent::Unknown(unknown) => {
                if self.options.deterministic {
                    // Unknown events are already raw.  Deterministic callers
                    // should submit them sorted; sorting is performed by the
                    // value API where the complete sequence is available.
                }
                self.append_to_message(&unknown.raw_bytes())
            }
        }
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, ProtoError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return self.fail(ProtoError::terminal(ProtoErrorKind::TrailingData));
        }
        if !self.stack.is_empty() || self.output.is_none() {
            return self.fail(ProtoError::terminal(ProtoErrorKind::TypeMismatch));
        }
        let bytes = self.output.take().expect("checked above");
        if bytes.len() > self.options.limits.max_output_bytes {
            return self.fail(ProtoError::new(ProtoErrorKind::LimitExceeded, bytes.len()));
        }
        if let Some(sink) = self.sink.as_mut()
            && sink.write_all(&bytes).and_then(|()| sink.flush()).is_err()
        {
            return self.fail(ProtoError::terminal(ProtoErrorKind::IoError));
        }
        self.finished = true;
        Ok(bytes)
    }
}

fn event_scalar(event: &Event, field: u32, output: &mut Vec<ProtoEvent>) -> Result<(), ProtoError> {
    match event {
        Event::Bool(value) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Varint));
            output.push(ProtoEvent::Varint(u64::from(*value)));
        }
        Event::Int(value) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Varint));
            output.push(ProtoEvent::Varint(
                i64::try_from(*value)
                    .map_err(|_| ProtoError::new(ProtoErrorKind::NumberRange, 0))?
                    as u64,
            ));
        }
        Event::UInt(value) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Varint));
            output
                .push(ProtoEvent::Varint(u64::try_from(*value).map_err(|_| {
                    ProtoError::new(ProtoErrorKind::NumberRange, 0)
                })?));
        }
        Event::Float32(bits) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Fixed32));
            output.push(ProtoEvent::Fixed32(*bits));
        }
        Event::Float64(bits) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Fixed64));
            output.push(ProtoEvent::Fixed64(*bits));
        }
        Event::Float(value) => {
            output.push(ProtoEvent::Field(field, ProtoWireType::Fixed64));
            output.push(ProtoEvent::Fixed64(value.to_bits()));
        }
        Event::String(value) => {
            if value.len() > ProtoLimits::default().max_string_bytes {
                return Err(ProtoError::new(ProtoErrorKind::LimitExceeded, 0));
            }
            output.push(ProtoEvent::StartLengthDelimited(field));
            output.push(ProtoEvent::Bytes(value.as_bytes().to_vec()));
            output.push(ProtoEvent::EndLengthDelimited);
        }
        Event::Bytes(value) => {
            output.push(ProtoEvent::StartLengthDelimited(field));
            output.push(ProtoEvent::Bytes(value.clone()));
            output.push(ProtoEvent::EndLengthDelimited);
        }
        Event::Null => return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0)),
        _ => return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, 0)),
    }
    Ok(())
}

fn serialization_events_to_proto(events: &[Event]) -> Result<Vec<ProtoEvent>, ProtoError> {
    fn encode_value(
        events: &[Event],
        mut index: usize,
        field: u32,
        output: &mut Vec<ProtoEvent>,
    ) -> Result<usize, ProtoError> {
        let Some(event) = events.get(index) else {
            return Err(ProtoError::new(ProtoErrorKind::UnexpectedEof, index));
        };
        match event {
            Event::StartArray(_) => {
                index += 1;
                while !matches!(events.get(index), Some(Event::EndArray)) {
                    index = encode_value(events, index, field, output)?;
                }
                Ok(index + 1)
            }
            Event::StartMap(_) => {
                index += 1;
                while !matches!(events.get(index), Some(Event::EndMap)) {
                    if !matches!(events.get(index), Some(Event::MapKey)) {
                        return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, index));
                    }
                    index += 1;
                    let mut entry = Vec::new();
                    entry.push(ProtoEvent::StartMessage("map-entry".to_owned()));
                    index = encode_value(events, index, 1, &mut entry)?;
                    index = encode_value(events, index, 2, &mut entry)?;
                    entry.push(ProtoEvent::EndMessage);
                    output.push(ProtoEvent::StartLengthDelimited(field));
                    output.extend(entry);
                    output.push(ProtoEvent::EndLengthDelimited);
                }
                Ok(index + 1)
            }
            Event::StartRecord { .. } => {
                output.push(ProtoEvent::StartLengthDelimited(field));
                output.push(ProtoEvent::StartMessage("nested".to_owned()));
                index += 1;
                let mut nested_field = 1u32;
                while !matches!(events.get(index), Some(Event::EndRecord)) {
                    if !matches!(events.get(index), Some(Event::Field(_))) {
                        return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, index));
                    }
                    index += 1;
                    index = encode_value(events, index, nested_field, output)?;
                    nested_field = nested_field.checked_add(1).ok_or_else(|| {
                        ProtoError::new(ProtoErrorKind::InvalidFieldNumber, index)
                    })?;
                }
                output.push(ProtoEvent::EndMessage);
                output.push(ProtoEvent::EndLengthDelimited);
                Ok(index + 1)
            }
            Event::Null => Ok(index + 1),
            Event::StartEnum { .. } | Event::EndEnum => {
                Err(ProtoError::new(ProtoErrorKind::TypeMismatch, index))
            }
            scalar => {
                event_scalar(scalar, field, output)?;
                Ok(index + 1)
            }
        }
    }

    let mut output = vec![ProtoEvent::StartMessage("root".to_owned())];
    if matches!(events.first(), Some(Event::StartRecord { .. })) {
        let mut index = 1;
        let mut field = 1u32;
        while !matches!(events.get(index), Some(Event::EndRecord)) {
            if !matches!(events.get(index), Some(Event::Field(_))) {
                return Err(ProtoError::new(ProtoErrorKind::TypeMismatch, index));
            }
            index += 1;
            index = encode_value(events, index, field, &mut output)?;
            field = field
                .checked_add(1)
                .ok_or_else(|| ProtoError::new(ProtoErrorKind::InvalidFieldNumber, index))?;
        }
        if index + 1 != events.len() {
            return Err(ProtoError::new(ProtoErrorKind::TrailingData, index));
        }
    } else {
        let end = encode_value(events, 0, 1, &mut output)?;
        if end != events.len() {
            return Err(ProtoError::new(ProtoErrorKind::TrailingData, end));
        }
    }
    output.push(ProtoEvent::EndMessage);
    Ok(output)
}

fn serialization_events_from_fields(
    fields: &[ParsedField],
    options: ProtoDecodeOptions,
) -> Result<Vec<Event>, ProtoError> {
    if fields.is_empty() {
        return Err(ProtoError::new(ProtoErrorKind::UnexpectedEof, 0));
    }
    let mut output = Vec::new();
    let first_number = fields[0].number;
    let repeated = fields
        .iter()
        .filter(|field| field.number == first_number)
        .count()
        > 1;
    if repeated {
        output.push(Event::StartArray(Some(fields.len())));
    }
    for field in fields {
        match field.wire_type {
            ProtoWireType::Varint => {
                let mut offset = 0;
                let (value, _) = read_varint(&field.payload, &mut offset, options)?;
                output.push(Event::UInt(u128::from(value)));
            }
            ProtoWireType::Fixed32 => output.push(Event::Float32(u32::from_le_bytes(
                field
                    .payload
                    .as_slice()
                    .try_into()
                    .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
            ))),
            ProtoWireType::Fixed64 => output.push(Event::Float64(u64::from_le_bytes(
                field
                    .payload
                    .as_slice()
                    .try_into()
                    .map_err(|_| ProtoError::new(ProtoErrorKind::InvalidLength, 0))?,
            ))),
            ProtoWireType::LengthDelimited => {
                if let Ok(text) = std::str::from_utf8(&field.payload) {
                    output.push(Event::String(text.to_owned()));
                } else {
                    output.push(Event::Bytes(field.payload.clone()));
                }
            }
            ProtoWireType::StartGroup => {
                if options.unknown_fields == ProtoUnknownPolicy::Preserve {
                    output.push(Event::Bytes(unknown_from(field).raw_bytes()));
                }
            }
            ProtoWireType::EndGroup => {
                return Err(ProtoError::new(ProtoErrorKind::InvalidGroup, 0));
            }
        }
    }
    if repeated {
        output.push(Event::EndArray);
    }
    Ok(output)
}

pub fn encode<T: Serialize>(value: &T, options: ProtoEncodeOptions) -> Result<Vec<u8>, ProtoError> {
    let events = serialization::serialize_value(
        value,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_events,
            max_bytes: options.limits.max_output_bytes,
            max_container_items: options.limits.max_repeated_items,
        },
    )
    .map_err(|_| ProtoError::new(ProtoErrorKind::TypeMismatch, 0))?;
    let proto_events = serialization_events_to_proto(&events)?;
    let mut writer = ProtoWriter::new(options);
    for event in proto_events {
        writer.write(event)?;
    }
    writer.finish()
}

pub fn decode<T: Deserialize>(input: &[u8], options: ProtoDecodeOptions) -> Result<T, ProtoError> {
    let fields = parse_fields(input, options)?;
    let events = serialization_events_from_fields(&fields, options)?;
    serialization::deserialize_value(
        &events,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_events,
            max_bytes: options.limits.max_message_bytes,
            max_container_items: options.limits.max_repeated_items,
        },
    )
    .map_err(|_| ProtoError::new(ProtoErrorKind::TypeMismatch, 0))
}

pub fn encode_deterministic<T: Serialize>(
    value: &T,
    limits: ProtoLimits,
) -> Result<Vec<u8>, ProtoError> {
    encode(
        value,
        ProtoEncodeOptions {
            limits,
            deterministic: true,
        },
    )
}

#[allow(non_snake_case)]
pub fn encodeDeterministic<T: Serialize>(
    value: &T,
    limits: ProtoLimits,
) -> Result<Vec<u8>, ProtoError> {
    encode_deterministic(value, limits)
}

pub fn validate<T>(input: &[u8], options: ProtoDecodeOptions) -> Result<(), ProtoError> {
    let _ = PhantomData::<T>;
    parse_fields(input, options).map(|_| ())
}

/// A deliberately small proto3 schema model.  The build tool can use it to
/// validate closed inputs without putting a parser or descriptor lookup in the
/// runtime hot path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoSchema {
    pub package: Option<String>,
    pub imports: Vec<String>,
    pub messages: Vec<ProtoMessageSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoMessageSchema {
    pub name: String,
    pub fields: Vec<ProtoFieldSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoFieldSchema {
    pub name: String,
    pub number: u32,
    pub type_name: String,
    pub repeated: bool,
    pub optional: bool,
}

fn build_error(
    kind: ProtoBuildErrorKind,
    schema: &str,
    path: impl Into<String>,
) -> ProtoBuildError {
    ProtoBuildError {
        kind,
        schema: schema.to_owned(),
        path: path.into(),
    }
}

fn schema_tokens(input: &str) -> Vec<String> {
    input
        .replace('{', " { ")
        .replace('}', " } ")
        .replace(';', " ; ")
        .replace('=', " = ")
        .replace('<', " < ")
        .replace('>', " > ")
        .replace(',', " , ")
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn parse_schema_field(
    tokens: &[String],
    cursor: &mut usize,
    message: &str,
    fields: &mut Vec<ProtoFieldSchema>,
    optional_default: bool,
) -> Result<(), ProtoBuildError> {
    let mut optional = optional_default;
    let mut repeated = false;
    if tokens.get(*cursor).map(String::as_str) == Some("optional") {
        optional = true;
        *cursor += 1;
    } else if tokens.get(*cursor).map(String::as_str) == Some("repeated") {
        repeated = true;
        *cursor += 1;
    }
    let (type_name, field_name, number, consumed) =
        if tokens.get(*cursor).map(String::as_str) == Some("map") {
            let key = tokens.get(*cursor + 2).cloned().ok_or_else(|| {
                build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    "map key",
                )
            })?;
            let value = tokens.get(*cursor + 4).cloned().ok_or_else(|| {
                build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    "map value",
                )
            })?;
            let field_name = tokens.get(*cursor + 6).cloned().ok_or_else(|| {
                build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    "map field",
                )
            })?;
            if tokens.get(*cursor + 1).map(String::as_str) != Some("<")
                || tokens.get(*cursor + 3).map(String::as_str) != Some(",")
                || tokens.get(*cursor + 5).map(String::as_str) != Some(">")
                || tokens.get(*cursor + 7).map(String::as_str) != Some("=")
            {
                return Err(build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    &field_name,
                ));
            }
            let number = tokens
                .get(*cursor + 8)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    build_error(
                        ProtoBuildErrorKind::ProtoFieldNumberConflict,
                        message,
                        &field_name,
                    )
                })?;
            (format!("Map[{key},{value}]"), field_name, number, 10)
        } else {
            let type_name = tokens.get(*cursor).cloned().ok_or_else(|| {
                build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    "field type",
                )
            })?;
            let field_name = tokens.get(*cursor + 1).cloned().ok_or_else(|| {
                build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    "field name",
                )
            })?;
            if tokens.get(*cursor + 2).map(String::as_str) != Some("=") {
                return Err(build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    message,
                    &field_name,
                ));
            }
            let number = tokens
                .get(*cursor + 3)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| {
                    build_error(
                        ProtoBuildErrorKind::ProtoFieldNumberConflict,
                        message,
                        &field_name,
                    )
                })?;
            (type_name, field_name, number, 5)
        };
    if number == 0
        || number > MAX_FIELD_NUMBER
        || (RESERVED_FIELD_START..=RESERVED_FIELD_END).contains(&number)
    {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoReservedReuse,
            message,
            &field_name,
        ));
    }
    if fields
        .iter()
        .any(|field| field.number == number || field.name == field_name)
    {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoFieldNumberConflict,
            message,
            &field_name,
        ));
    }
    fields.push(ProtoFieldSchema {
        name: field_name,
        number,
        type_name,
        repeated,
        optional,
    });
    *cursor += consumed;
    if tokens.get(*cursor).map(String::as_str) == Some(";") {
        *cursor += 1;
    }
    Ok(())
}

/// Parse the proto3 subset needed by the generator contract: package and
/// messages with scalar, repeated and optional fields. Unsupported constructs
/// fail with a stable build error rather than being silently approximated.
pub fn parse_schema(input: &str, limits: ProtoLimits) -> Result<ProtoSchema, ProtoBuildError> {
    if input.len() > limits.max_schema_bytes {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "schema",
            "max_schema_bytes",
        ));
    }
    let tokens = schema_tokens(input);
    if !tokens.iter().any(|token| token == "syntax") || !input.contains("\"proto3\"") {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoSyntaxUnsupported,
            "schema",
            "syntax",
        ));
    }
    let mut package = None;
    let mut imports = Vec::new();
    let mut messages = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "package" => {
                let name = tokens.get(index + 1).cloned().ok_or_else(|| {
                    build_error(
                        ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                        "schema",
                        "package",
                    )
                })?;
                package = Some(name);
                index += 3;
            }
            "import" => {
                let raw = tokens.get(index + 1).cloned().ok_or_else(|| {
                    build_error(
                        ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                        "schema",
                        "import",
                    )
                })?;
                let path = raw.trim_matches('"').to_owned();
                if path.is_empty()
                    || path.starts_with('/')
                    || path.split('/').any(|part| part == "..")
                {
                    return Err(build_error(
                        ProtoBuildErrorKind::ProtoImportNotDeclared,
                        "schema",
                        path,
                    ));
                }
                imports.push(path);
                index += 3;
            }
            "message" => {
                let name = tokens.get(index + 1).cloned().ok_or_else(|| {
                    build_error(
                        ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                        "schema",
                        "message",
                    )
                })?;
                let mut cursor = index + 3;
                let mut fields = Vec::new();
                while cursor < tokens.len() && tokens[cursor] != "}" {
                    if tokens[cursor] == "oneof" {
                        cursor += 3; // oneof name {
                        while cursor < tokens.len() && tokens[cursor] != "}" {
                            parse_schema_field(&tokens, &mut cursor, &name, &mut fields, true)?;
                        }
                        if tokens.get(cursor).map(String::as_str) != Some("}") {
                            return Err(build_error(
                                ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                                &name,
                                "oneof",
                            ));
                        }
                        cursor += 1;
                    } else {
                        parse_schema_field(&tokens, &mut cursor, &name, &mut fields, false)?;
                    }
                }
                if cursor >= tokens.len() {
                    return Err(build_error(
                        ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                        &name,
                        "message",
                    ));
                }
                messages.push(ProtoMessageSchema { name, fields });
                index = cursor + 1;
            }
            "enum" => {
                // Enum values are open Int32 values at runtime.  The compact
                // schema model does not need to retain every known name, but
                // it must consume the declaration deterministically.
                let mut depth = 0usize;
                while index < tokens.len() {
                    if tokens[index] == "{" {
                        depth += 1;
                    } else if tokens[index] == "}" {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            index += 1;
                            break;
                        }
                    }
                    index += 1;
                }
            }
            "service" | "extend" | "extensions" | "reserved" | "oneof" => {
                return Err(build_error(
                    ProtoBuildErrorKind::ProtoSyntaxUnsupported,
                    "schema",
                    tokens[index].clone(),
                ));
            }
            _ => index += 1,
        }
    }
    if messages.len() > limits.max_generated_types {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "schema",
            "max_generated_types",
        ));
    }
    if imports.len() > limits.max_imports {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "schema",
            "max_imports",
        ));
    }
    Ok(ProtoSchema {
        package,
        imports,
        messages,
    })
}

/// Parse a closed schema graph.  Every import must name another input exactly;
/// there is no current-directory, environment, network or installed-descriptor
/// fallback.  The returned order is the caller's stable input order.
pub fn parse_schema_graph(
    inputs: &[(&str, &str)],
    limits: ProtoLimits,
) -> Result<Vec<(String, ProtoSchema)>, ProtoBuildError> {
    if inputs.len() > limits.max_imports.saturating_add(1) {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "graph",
            "max_imports",
        ));
    }
    let mut paths = BTreeSet::new();
    for (path, _) in inputs {
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|part| part == "..")
            || !paths.insert((*path).to_owned())
        {
            return Err(build_error(
                ProtoBuildErrorKind::ProtoImportNotDeclared,
                path,
                "path",
            ));
        }
    }
    let mut result = Vec::with_capacity(inputs.len());
    for (path, source) in inputs {
        let schema = parse_schema(source, limits)?;
        for import in &schema.imports {
            if !paths.contains(import) {
                return Err(build_error(
                    ProtoBuildErrorKind::ProtoImportNotDeclared,
                    path,
                    import,
                ));
            }
        }
        result.push(((*path).to_owned(), schema));
    }
    Ok(result)
}

pub fn check_evolution(previous: &ProtoSchema, next: &ProtoSchema) -> Result<(), ProtoBuildError> {
    for old_message in &previous.messages {
        let Some(new_message) = next
            .messages
            .iter()
            .find(|message| message.name == old_message.name)
        else {
            return Err(build_error(
                ProtoBuildErrorKind::ProtoSchemaDrift,
                &old_message.name,
                "message removed",
            ));
        };
        for old_field in &old_message.fields {
            let Some(new_field) = new_message
                .fields
                .iter()
                .find(|field| field.number == old_field.number)
            else {
                continue;
            };
            if old_field.type_name != new_field.type_name
                || old_field.repeated != new_field.repeated
            {
                return Err(build_error(
                    ProtoBuildErrorKind::ProtoWireIncompatible,
                    &old_message.name,
                    &old_field.name,
                ));
            }
        }
    }
    Ok(())
}

/// Generate stable, ordinary Tondo source for the bounded schema model.  The
/// generated text is intentionally boring: field numbers remain explicit in
/// a companion comment and the nominal record layout is deterministic by
/// message name and field number.  The build tool can hash this output without
/// loading a runtime descriptor.
pub fn generate_tondo(
    schema: &ProtoSchema,
    limits: ProtoLimits,
) -> Result<String, ProtoBuildError> {
    if schema.messages.len() > limits.max_generated_types {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "schema",
            "max_generated_types",
        ));
    }
    let mut messages = schema.messages.clone();
    messages.sort_by(|left, right| left.name.cmp(&right.name));
    let mut output = String::new();
    if let Some(package) = &schema.package {
        output.push_str("// package ");
        output.push_str(package);
        output.push('\n');
    }
    for message in messages {
        output.push_str("pub record ");
        output.push_str(&message.name);
        output.push_str(" {\n");
        let mut fields = message.fields.clone();
        fields.sort_by_key(|field| field.number);
        for field in fields {
            output.push_str("    // protobuf field ");
            output.push_str(&field.number.to_string());
            output.push('\n');
            output.push_str("    pub ");
            output.push_str(&field.name);
            output.push_str(": ");
            if field.repeated {
                output.push_str("Array[");
            } else if field.optional {
                output.push_str("Option[");
            }
            output.push_str(&field.type_name);
            if field.repeated || field.optional {
                output.push(']');
            }
            output.push('\n');
        }
        output.push_str("}\n\n");
    }
    if output.len() > limits.max_generated_bytes {
        return Err(build_error(
            ProtoBuildErrorKind::ProtoGenerationLimit,
            "schema",
            "max_generated_bytes",
        ));
    }
    Ok(output)
}

#[allow(non_snake_case)]
pub fn generateTondo(schema: &ProtoSchema, limits: ProtoLimits) -> Result<String, ProtoBuildError> {
    generate_tondo(schema, limits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ProtoDecodeOptions {
        ProtoDecodeOptions::default()
    }

    #[test]
    fn reader_emits_explicit_events_for_all_wire_widths() {
        let input = [
            0x08, 0x96, 0x01, 0x11, 1, 2, 3, 4, 5, 6, 7, 8, 0x1a, 2, b'o', b'k', 0x25, 1, 2, 3, 4,
        ];
        let mut reader = ProtoReader::<()>::from_bytes(&input, options()).unwrap();
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::StartMessage("root".to_owned()))
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Field(1, ProtoWireType::Varint))
        );
        assert_eq!(reader.next().unwrap(), Some(ProtoEvent::Varint(150)));
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Field(2, ProtoWireType::Fixed64))
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Fixed64(0x0807_0605_0403_0201))
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::StartLengthDelimited(3))
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Bytes(b"ok".to_vec()))
        );
        assert_eq!(reader.next().unwrap(), Some(ProtoEvent::EndLengthDelimited));
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Field(4, ProtoWireType::Fixed32))
        );
        assert_eq!(
            reader.next().unwrap(),
            Some(ProtoEvent::Fixed32(0x0403_0201))
        );
        assert_eq!(reader.next().unwrap(), Some(ProtoEvent::EndMessage));
        assert_eq!(reader.next().unwrap(), None);
        assert_eq!(reader.next().unwrap(), None);
        reader.finish().unwrap();

        let mut chunks =
            ProtoReader::<()>::fromChunks([vec![0x08], vec![0x96], vec![0x01]], options()).unwrap();
        let mut chunk_events = Vec::new();
        while let Some(event) = chunks.next().unwrap() {
            chunk_events.push(event);
        }
        let mut whole_reader =
            ProtoReader::<()>::from_bytes(&[0x08, 0x96, 0x01], options()).unwrap();
        let mut whole_events = Vec::new();
        while let Some(event) = whole_reader.next().unwrap() {
            whole_events.push(event);
        }
        assert_eq!(whole_events, chunk_events);
    }

    #[test]
    fn writer_round_trips_events_and_becomes_terminal_after_finish() {
        let mut writer = ProtoWriter::new(ProtoEncodeOptions::default());
        for event in [
            ProtoEvent::StartMessage("root".to_owned()),
            ProtoEvent::Field(1, ProtoWireType::Varint),
            ProtoEvent::Varint(150),
            ProtoEvent::StartLengthDelimited(2),
            ProtoEvent::Bytes(b"ok".to_vec()),
            ProtoEvent::EndLengthDelimited,
            ProtoEvent::EndMessage,
        ] {
            writer.write(event).unwrap();
        }
        let bytes = writer.finish().unwrap();
        assert_eq!(bytes, vec![0x08, 0x96, 0x01, 0x12, 2, b'o', b'k']);
        assert_eq!(
            writer.finish(),
            Err(ProtoError::terminal(ProtoErrorKind::TrailingData))
        );
    }

    #[test]
    fn dynamic_message_preserves_nested_values_and_deterministic_order() {
        let fields = vec![
            ProtoField {
                number: 2,
                value: ProtoValue::Varint(7),
            },
            ProtoField {
                number: 1,
                value: ProtoValue::Message(vec![ProtoField {
                    number: 1,
                    value: ProtoValue::Bytes(b"x".to_vec()),
                }]),
            },
        ];
        let bytes = encode_message(
            &fields,
            ProtoEncodeOptions {
                deterministic: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bytes, vec![0x0a, 3, 0x0a, 1, b'x', 0x10, 7]);
        let decoded = decode_message(&bytes, options()).unwrap();
        assert_eq!(
            decoded,
            vec![
                ProtoField {
                    number: 1,
                    value: ProtoValue::Bytes(vec![0x0a, 1, b'x'])
                },
                ProtoField {
                    number: 2,
                    value: ProtoValue::Varint(7)
                },
            ]
        );
    }

    #[test]
    fn unknown_groups_keep_exact_raw_bytes_and_limits_are_enforced() {
        let input = [0x53, 0x08, 0x96, 0x01, 0x54];
        let fields = parse_fields(&input, options()).unwrap();
        let unknown = unknown_from(&fields[0]);
        assert_eq!(unknown.number, 10);
        assert_eq!(unknown.wire_type, ProtoWireType::StartGroup);
        assert_eq!(unknown.raw_bytes(), input);
        let small = ProtoDecodeOptions {
            limits: ProtoLimits {
                max_message_bytes: 2,
                ..Default::default()
            },
            ..options()
        };
        assert_eq!(
            parse_fields(&input, small).unwrap_err().kind,
            ProtoErrorKind::LimitExceeded
        );
    }

    #[test]
    fn malformed_varints_and_wire_types_fail_without_partial_events() {
        assert_eq!(
            parse_fields(&[0x80], options()).unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );
        assert_eq!(
            parse_fields(&[0x0b], options()).unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );
        assert_eq!(
            parse_fields(
                &[
                    0x08, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02
                ],
                options()
            )
            .unwrap_err()
            .kind,
            ProtoErrorKind::InvalidVarint
        );
        assert_eq!(
            parse_fields(&[0x0f], options()).unwrap_err().kind,
            ProtoErrorKind::InvalidWireType
        );
    }

    #[test]
    fn schema_parser_is_bounded_and_evolution_rejects_wire_changes() {
        let old = parse_schema(
            "syntax = \"proto3\"; package demo; message User { string name = 1; }",
            ProtoLimits::default(),
        )
        .unwrap();
        let next = parse_schema(
            "syntax = \"proto3\"; package demo; message User { bytes name = 1; }",
            ProtoLimits::default(),
        )
        .unwrap();
        assert_eq!(
            check_evolution(&old, &next).unwrap_err().kind,
            ProtoBuildErrorKind::ProtoWireIncompatible
        );
        let generated = generate_tondo(&old, ProtoLimits::default()).unwrap();
        assert!(generated.contains("protobuf field 1"));
        assert_eq!(
            generated,
            generateTondo(&old, ProtoLimits::default()).unwrap()
        );
        assert!(
            parse_schema(
                "syntax = \"proto2\"; message User {}",
                ProtoLimits::default()
            )
            .is_err()
        );
        let graph = parse_schema_graph(
            &[
                ("common.proto", "syntax = \"proto3\"; message Common {}"),
                (
                    "user.proto",
                    "syntax = \"proto3\"; import \"common.proto\"; message User {}",
                ),
            ],
            ProtoLimits::default(),
        )
        .unwrap();
        assert_eq!(graph[1].1.imports, vec!["common.proto"]);
        assert!(
            parse_schema_graph(
                &[(
                    "user.proto",
                    "syntax = \"proto3\"; import \"missing.proto\"; message User {}"
                )],
                ProtoLimits::default(),
            )
            .is_err()
        );
        let rich = parse_schema(
            "syntax = \"proto3\"; enum Role { UNKNOWN = 0; ADMIN = 1; } message User { map<string, int32> scores = 1; oneof identity { string name = 2; int64 id = 3; } }",
            ProtoLimits::default(),
        )
        .unwrap();
        assert_eq!(rich.messages[0].fields[0].type_name, "Map[string,int32]");
        assert!(rich.messages[0].fields[1].optional);
    }

    #[test]
    fn typed_scalars_use_the_common_static_event_protocol() {
        let bytes = encode(&150_i64, ProtoEncodeOptions::default()).unwrap();
        assert_eq!(bytes, vec![0x08, 0x96, 0x01]);
        let value: i64 = decode(&bytes, options()).unwrap();
        assert_eq!(value, 150);
        let deterministic = encodeDeterministic(&150_i64, ProtoLimits::default()).unwrap();
        assert_eq!(deterministic, bytes);
        validate::<i64>(&bytes, options()).unwrap();
        assert_eq!(descriptor::<i64>().name(), "i64");
    }

    #[test]
    fn unknown_fields_api_is_explicitly_owned() {
        let mut fields = UnknownFields::default();
        fields.push(UnknownField {
            number: 99,
            wire_type: ProtoWireType::Varint,
            tag_bytes: vec![0x98, 0x06],
            payload_bytes: vec![1],
        });
        assert_eq!(fields.count(), 1);
        fields[0].payload_bytes[0] = 2;
        assert_eq!(fields[0].payload_bytes, vec![2]);
        fields.discardUnknown();
        assert!(fields.is_empty());
    }

    struct EventSequence(Vec<Event>);

    impl Serialize for EventSequence {
        fn serialize<S: serialization::Serializer<Error = serialization::SerializationError>>(
            &self,
            serializer: &mut S,
        ) -> Result<(), serialization::SerializationError> {
            for event in &self.0 {
                serializer.write_event(event.clone())?;
            }
            Ok(())
        }
    }

    struct FailingReader;

    impl std::io::Read for FailingReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("read failure"))
        }
    }

    struct FailingWriter;

    impl std::io::Write for FailingWriter {
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("write failure"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("flush failure"))
        }
    }

    #[test]
    fn public_error_paths_limits_and_dynamic_values_are_exercised() {
        let mut path = ProtoPath::root();
        path.push(ProtoPathSegment::Message("User".into()));
        path.push(ProtoPathSegment::FieldNumber(1));
        path.push(ProtoPathSegment::RepeatedIndex(2));
        path.push(ProtoPathSegment::MapKey("role".into()));
        path.push(ProtoPathSegment::MapValue);
        path.push(ProtoPathSegment::OneofCase("identity".into()));
        path.push(ProtoPathSegment::UnknownField(99));
        assert_eq!(path.segments().len(), 7);
        assert_eq!(
            path.to_string(),
            "$.User.field[1][2][key=\"role\"][value].oneof[identity].unknown[99]"
        );

        let error = ProtoError::new(ProtoErrorKind::SchemaMismatch, 3);
        assert!(error.to_string().contains("SchemaMismatch"));
        assert!(
            ProtoError::terminal(ProtoErrorKind::IoError)
                .to_string()
                .contains("IoError")
        );
        let build = build_error(ProtoBuildErrorKind::ProtoNameCollision, "User", "field");
        assert!(build.to_string().contains("ProtoNameCollision"));

        let defaults = ProtoLimits::default();
        for limits in [
            ProtoLimits {
                max_schema_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_message_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_depth: 0,
                ..defaults
            },
            ProtoLimits {
                max_fields: 0,
                ..defaults
            },
            ProtoLimits {
                max_repeated_items: 0,
                ..defaults
            },
            ProtoLimits {
                max_map_entries: 0,
                ..defaults
            },
            ProtoLimits {
                max_string_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_bytes_field_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_packed_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_unknown_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_varint_bytes: 0,
                ..defaults
            },
            ProtoLimits {
                max_varint_bytes: 11,
                ..defaults
            },
            ProtoLimits {
                max_events: 0,
                ..defaults
            },
            ProtoLimits {
                max_output_bytes: 0,
                ..defaults
            },
        ] {
            assert_eq!(
                checked_limits(limits).unwrap_err().kind,
                ProtoErrorKind::LimitExceeded
            );
        }

        assert_eq!(
            field_number(0).unwrap_err(),
            ProtoErrorKind::InvalidFieldNumber
        );
        assert_eq!(
            field_number((MAX_FIELD_NUMBER as u64 + 1) << 3).unwrap_err(),
            ProtoErrorKind::InvalidFieldNumber
        );
        assert_eq!(
            field_number((19_000_u64) << 3).unwrap_err(),
            ProtoErrorKind::InvalidFieldNumber
        );
        assert_eq!(ProtoWireType::from_byte(6), None);
        let mut cursor = 0;
        let strict = ProtoDecodeOptions {
            reject_non_minimal_varints: true,
            ..options()
        };
        assert_eq!(
            read_varint(&[0x80, 0x00], &mut cursor, strict)
                .unwrap_err()
                .kind,
            ProtoErrorKind::InvalidVarint
        );

        let unknown = UnknownField {
            number: 4,
            wire_type: ProtoWireType::Varint,
            tag_bytes: vec![0x20],
            payload_bytes: vec![1],
        };
        let fields = vec![
            ProtoField {
                number: 1,
                value: ProtoValue::Varint(7),
            },
            ProtoField {
                number: 2,
                value: ProtoValue::Fixed32(3),
            },
            ProtoField {
                number: 3,
                value: ProtoValue::Fixed64(4),
            },
            ProtoField {
                number: 4,
                value: ProtoValue::Bytes(vec![1, 2]),
            },
            ProtoField {
                number: 5,
                value: ProtoValue::Message(vec![ProtoField {
                    number: 1,
                    value: ProtoValue::Varint(9),
                }]),
            },
            ProtoField {
                number: 6,
                value: ProtoValue::Unknown(unknown),
            },
        ];
        let ordinary = encode_message(&fields, ProtoEncodeOptions::default()).unwrap();
        assert!(!ordinary.is_empty());
        assert!(
            encode_message(
                &[ProtoField {
                    number: 0,
                    value: ProtoValue::Varint(1)
                }],
                ProtoEncodeOptions::default()
            )
            .is_err()
        );
        assert!(
            encode_message(
                &[ProtoField {
                    number: 1,
                    value: ProtoValue::Bytes(vec![1, 2])
                }],
                ProtoEncodeOptions {
                    limits: ProtoLimits {
                        max_bytes_field_bytes: 1,
                        ..defaults
                    },
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            encode_message(
                &fields,
                ProtoEncodeOptions {
                    limits: ProtoLimits {
                        max_output_bytes: 1,
                        ..defaults
                    },
                    ..Default::default()
                }
            )
            .is_err()
        );
        let decoded = decode_message(&ordinary, options()).unwrap();
        assert_eq!(decoded.len(), 6);

        let discard = ProtoDecodeOptions {
            unknown_fields: ProtoUnknownPolicy::Discard,
            ..options()
        };
        let mut discard_reader =
            ProtoReader::<()>::from_bytes(&[0x53, 0x08, 1, 0x54], discard).unwrap();
        let mut discard_events = Vec::new();
        while let Some(event) = discard_reader.next().unwrap() {
            discard_events.push(event);
        }
        assert!(
            !discard_events
                .iter()
                .any(|event| matches!(event, ProtoEvent::Unknown(_)))
        );
        assert!(ProtoReader::<()>::from_reader(FailingReader, options()).is_err());
        let mut bad_reader = ProtoReader::<()>::from_bytes(&[0x80], options()).unwrap();
        assert_eq!(
            bad_reader.next().unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );
        assert_eq!(
            bad_reader.next().unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );
        assert!(bad_reader.own(ProtoEvent::EndMessage).is_err());

        let sequence = EventSequence(vec![Event::String("hello".into())]);
        assert_eq!(
            encode(&sequence, ProtoEncodeOptions::default()).unwrap(),
            vec![0x0a, 5, b'h', b'e', b'l', b'l', b'o']
        );
        let array = EventSequence(vec![
            Event::StartArray(Some(2)),
            Event::Int(1),
            Event::Int(2),
            Event::EndArray,
        ]);
        assert_eq!(
            encode(&array, ProtoEncodeOptions::default()).unwrap(),
            vec![0x08, 1, 0x08, 2]
        );
        let map = EventSequence(vec![
            Event::StartMap(Some(1)),
            Event::MapKey,
            Event::String("k".into()),
            Event::UInt(2),
            Event::EndMap,
        ]);
        assert!(encode(&map, ProtoEncodeOptions::default()).is_ok());
        let unsupported = EventSequence(vec![
            Event::StartEnum {
                name: "E".into(),
                variant: "V".into(),
            },
            Event::EndEnum,
        ]);
        assert_eq!(
            encode(&unsupported, ProtoEncodeOptions::default())
                .unwrap_err()
                .kind,
            ProtoErrorKind::TypeMismatch
        );
        assert_eq!(
            serialization_events_to_proto(&[Event::Int(1), Event::Int(2)])
                .unwrap_err()
                .kind,
            ProtoErrorKind::TrailingData
        );
        assert!(decode::<String>(&[0x0a, 1, b'x'], options()).is_ok());
        assert!(decode::<Vec<i64>>(&[0x08, 1, 0x08, 2], options()).is_ok());

        let mut alias_reader = ProtoReader::<u32>::fromBytes(&[0x08, 1], options()).unwrap();
        assert!(alias_reader.next().unwrap().is_some());
        let mut alias_stream =
            ProtoReader::<u32>::fromReader(std::io::Cursor::new(vec![0x08, 1]), options()).unwrap();
        alias_stream.finish().unwrap();

        let nested_groups = [0x53, 0x5b, 0x08, 1, 0x5c, 0x54];
        assert_eq!(parse_fields(&nested_groups, options()).unwrap().len(), 1);
        let mut failing = FailingWriter;
        let _ = std::io::Write::write(&mut failing, b"direct");
    }

    #[test]
    fn wire_and_typed_edge_matrix_covers_bounded_branches() {
        let mut overflow_offset = usize::MAX;
        assert_eq!(
            take(&[], &mut overflow_offset, 1).unwrap_err().kind,
            ProtoErrorKind::InvalidLength
        );
        let mut eof_offset = 0;
        assert_eq!(
            take(&[], &mut eof_offset, 1).unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );

        let mut cursor = 0;
        assert_eq!(
            read_varint(&[0x96, 0x01], &mut cursor, options())
                .unwrap()
                .0,
            150
        );
        let mut cursor = 0;
        assert_eq!(
            read_varint(&[0x80; 10], &mut cursor, options())
                .unwrap_err()
                .kind,
            ProtoErrorKind::InvalidVarint
        );

        for input in [
            vec![0x53, 0x11, 1, 2, 3, 4, 5, 6, 7, 8, 0x54],
            vec![0x53, 0x15, 1, 2, 3, 4, 0x54],
            vec![0x53, 0x12, 2, 1, 2, 0x54],
        ] {
            assert_eq!(parse_fields(&input, options()).unwrap().len(), 1);
        }
        assert_eq!(
            parse_fields(&[0x53, 0x5c], options()).unwrap_err().kind,
            ProtoErrorKind::InvalidGroup
        );
        assert_eq!(
            parse_fields(
                &[0x53, 0x5b, 0x54, 0x5c, 0x54],
                ProtoDecodeOptions {
                    limits: ProtoLimits {
                        max_depth: 1,
                        ..Default::default()
                    },
                    ..options()
                }
            )
            .unwrap_err()
            .kind,
            ProtoErrorKind::LimitExceeded
        );
        assert_eq!(
            parse_fields(&[0x01], options()).unwrap_err().kind,
            ProtoErrorKind::InvalidTag
        );
        assert_eq!(
            parse_fields(&[0x0c], options()).unwrap_err().kind,
            ProtoErrorKind::InvalidGroup
        );
        assert_eq!(
            parse_fields(
                &[0x0a, 100],
                ProtoDecodeOptions {
                    limits: ProtoLimits {
                        max_message_bytes: 10,
                        ..Default::default()
                    },
                    ..options()
                }
            )
            .unwrap_err()
            .kind,
            ProtoErrorKind::LimitExceeded
        );
        assert_eq!(
            parse_fields(
                &[0x0a, 2, 1, 2],
                ProtoDecodeOptions {
                    limits: ProtoLimits {
                        max_bytes_field_bytes: 1,
                        ..Default::default()
                    },
                    ..options()
                }
            )
            .unwrap_err()
            .kind,
            ProtoErrorKind::LimitExceeded
        );
        assert_eq!(
            parse_fields(
                &[0x08, 1, 0x10, 2],
                ProtoDecodeOptions {
                    limits: ProtoLimits {
                        max_fields: 1,
                        ..Default::default()
                    },
                    ..options()
                }
            )
            .unwrap_err()
            .kind,
            ProtoErrorKind::LimitExceeded
        );

        let mut event_limit = ProtoReader::<()>::from_bytes(
            &[0x08, 1],
            ProtoDecodeOptions {
                limits: ProtoLimits {
                    max_events: 1,
                    ..Default::default()
                },
                ..options()
            },
        )
        .unwrap();
        assert_eq!(
            event_limit.next().unwrap_err().kind,
            ProtoErrorKind::LimitExceeded
        );
        let mut preserved_group = ProtoReader::<()>::from_bytes(&[0x53, 0x54], options()).unwrap();
        let mut preserved = Vec::new();
        while let Some(event) = preserved_group.next().unwrap() {
            preserved.push(event);
        }
        assert!(
            preserved
                .iter()
                .any(|event| matches!(event, ProtoEvent::Unknown(_)))
        );
        assert!(
            ProtoReader::<()>::fromReader(
                std::io::Cursor::new(vec![1, 2]),
                ProtoDecodeOptions {
                    limits: ProtoLimits {
                        max_message_bytes: 1,
                        ..Default::default()
                    },
                    ..options()
                }
            )
            .is_err()
        );
        let mut alias_bad = ProtoReader::<u32>::fromBytes(&[0x80], options()).unwrap();
        assert_eq!(
            alias_bad.next().unwrap_err().kind,
            ProtoErrorKind::UnexpectedEof
        );

        let mut fixed = ProtoWriter::new(ProtoEncodeOptions::default());
        fixed
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        fixed
            .write(ProtoEvent::Field(1, ProtoWireType::Fixed32))
            .unwrap();
        fixed.write(ProtoEvent::Fixed32(7)).unwrap();
        fixed
            .write(ProtoEvent::Field(2, ProtoWireType::Fixed64))
            .unwrap();
        fixed.write(ProtoEvent::Fixed64(8)).unwrap();
        fixed.write(ProtoEvent::EndMessage).unwrap();
        assert!(!fixed.finish().unwrap().is_empty());
        let mut limited = ProtoWriter::new(ProtoEncodeOptions {
            limits: ProtoLimits {
                max_output_bytes: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        limited
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        limited
            .write(ProtoEvent::Field(1, ProtoWireType::Varint))
            .unwrap();
        assert_eq!(
            limited.write(ProtoEvent::Varint(1)).unwrap_err().kind,
            ProtoErrorKind::LimitExceeded
        );

        for event in [
            Event::Bool(true),
            Event::Int(-1),
            Event::UInt(2),
            Event::Float32(3),
            Event::Float64(4),
            Event::Float(5.0),
            Event::Bytes(vec![1]),
        ] {
            assert!(encode(&EventSequence(vec![event]), ProtoEncodeOptions::default()).is_ok());
        }
        assert!(
            encode(
                &EventSequence(vec![Event::Null]),
                ProtoEncodeOptions::default()
            )
            .unwrap()
            .is_empty()
        );
        let record = EventSequence(vec![
            Event::StartRecord {
                name: "R".into(),
                fields: Some(1),
            },
            Event::Field("x".into()),
            Event::Int(1),
            Event::EndRecord,
        ]);
        assert!(encode(&record, ProtoEncodeOptions::default()).is_ok());
        let malformed_map = EventSequence(vec![
            Event::StartMap(Some(1)),
            Event::String("not-a-key-marker".into()),
        ]);
        assert_eq!(
            encode(&malformed_map, ProtoEncodeOptions::default())
                .unwrap_err()
                .kind,
            ProtoErrorKind::TypeMismatch
        );

        let schema = parse_schema(
            "syntax = \"proto3\"; message User { optional string nickname = 1; repeated int32 ids = 2; }",
            ProtoLimits::default(),
        )
        .unwrap();
        assert!(schema.messages[0].fields[0].optional);
        assert!(schema.messages[0].fields[1].repeated);
    }

    #[test]
    fn writer_protocol_covers_nested_packed_sink_and_terminal_errors() {
        let mut writer = ProtoWriter::toWriter(Vec::<u8>::new(), ProtoEncodeOptions::default());
        for event in [
            ProtoEvent::StartMessage("root".into()),
            ProtoEvent::StartLengthDelimited(1),
            ProtoEvent::StartMessage("nested".into()),
            ProtoEvent::Field(1, ProtoWireType::Varint),
            ProtoEvent::Varint(1),
            ProtoEvent::EndMessage,
            ProtoEvent::EndLengthDelimited,
            ProtoEvent::StartPacked(2),
            ProtoEvent::Bytes(vec![1, 2]),
            ProtoEvent::EndPacked,
            ProtoEvent::Unknown(UnknownField {
                number: 9,
                wire_type: ProtoWireType::Varint,
                tag_bytes: vec![0x48],
                payload_bytes: vec![1],
            }),
            ProtoEvent::EndMessage,
        ] {
            writer.write(event).unwrap();
        }
        assert!(!writer.finish().unwrap().is_empty());

        let mut sink_writer = ProtoWriter::to_writer(FailingWriter, ProtoEncodeOptions::default());
        sink_writer
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        sink_writer.write(ProtoEvent::EndMessage).unwrap();
        assert_eq!(
            sink_writer.finish().unwrap_err().kind,
            ProtoErrorKind::IoError
        );

        let mut incomplete = ProtoWriter::new(ProtoEncodeOptions::default());
        assert_eq!(
            incomplete.finish().unwrap_err().kind,
            ProtoErrorKind::TypeMismatch
        );
        let mut before_start = ProtoWriter::new(ProtoEncodeOptions::default());
        assert_eq!(
            before_start.write(ProtoEvent::Varint(1)).unwrap_err().kind,
            ProtoErrorKind::TypeMismatch
        );
        let mut wrong = ProtoWriter::new(ProtoEncodeOptions::default());
        wrong
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        assert!(
            wrong
                .write(ProtoEvent::StartMessage("nested".into()))
                .is_err()
        );
        let mut pending = ProtoWriter::new(ProtoEncodeOptions::default());
        pending
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        pending
            .write(ProtoEvent::Field(1, ProtoWireType::Varint))
            .unwrap();
        assert!(pending.write(ProtoEvent::EndMessage).is_err());
        let mut scalar_mismatch = ProtoWriter::new(ProtoEncodeOptions::default());
        scalar_mismatch
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        scalar_mismatch
            .write(ProtoEvent::Field(1, ProtoWireType::Fixed32))
            .unwrap();
        assert!(scalar_mismatch.write(ProtoEvent::Varint(1)).is_err());
        let mut bad_length = ProtoWriter::new(ProtoEncodeOptions::default());
        bad_length
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        assert!(bad_length.write(ProtoEvent::EndLengthDelimited).is_err());
        let mut bad_bytes = ProtoWriter::new(ProtoEncodeOptions::default());
        bad_bytes
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        assert!(bad_bytes.write(ProtoEvent::Bytes(vec![1])).is_err());
        let mut bad_packed = ProtoWriter::new(ProtoEncodeOptions::default());
        bad_packed
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        assert!(bad_packed.write(ProtoEvent::EndPacked).is_err());
        let mut depth = ProtoWriter::new(ProtoEncodeOptions {
            limits: ProtoLimits {
                max_depth: 1,
                ..Default::default()
            },
            ..Default::default()
        });
        depth
            .write(ProtoEvent::StartMessage("root".into()))
            .unwrap();
        depth.write(ProtoEvent::StartLengthDelimited(1)).unwrap();
        assert!(
            depth
                .write(ProtoEvent::StartMessage("nested".into()))
                .is_err()
        );
    }

    #[test]
    fn schema_error_matrix_and_generation_limits_are_explicit() {
        let limits = ProtoLimits::default();
        for source in [
            "message User {}",
            "syntax = \"proto2\"; message User {}",
            "syntax = \"proto3\"; import \"../escape.proto\"; message User {}",
            "syntax = \"proto3\"; message User { string name = 0; }",
            "syntax = \"proto3\"; message User { string name = 19000; }",
            "syntax = \"proto3\"; message User { string name = 1; string name = 2; }",
            "syntax = \"proto3\"; message User { map<string int32> values = 1; }",
            "syntax = \"proto3\"; message User { oneof identity { string name = 1; ",
        ] {
            assert!(parse_schema(source, limits).is_err());
        }
        assert!(parse_schema("syntax = \"proto3\"; service S {}", limits).is_err());
        let schema = parse_schema(
            "syntax = \"proto3\"; message User { string name = 1; }",
            limits,
        )
        .unwrap();
        assert!(
            generate_tondo(
                &schema,
                ProtoLimits {
                    max_generated_types: 0,
                    ..limits
                }
            )
            .is_err()
        );
        assert!(
            generate_tondo(
                &schema,
                ProtoLimits {
                    max_generated_bytes: 1,
                    ..limits
                }
            )
            .is_err()
        );
        let next = ProtoSchema {
            package: None,
            imports: Vec::new(),
            messages: Vec::new(),
        };
        assert_eq!(
            check_evolution(&schema, &next).unwrap_err().kind,
            ProtoBuildErrorKind::ProtoSchemaDrift
        );
        assert!(
            parse_schema_graph(&[("", "syntax = \"proto3\"; message User {}")], limits).is_err()
        );
        assert!(
            parse_schema_graph(
                &[("/user.proto", "syntax = \"proto3\"; message User {}")],
                limits
            )
            .is_err()
        );
        assert!(
            parse_schema_graph(
                &[
                    ("user.proto", "syntax = \"proto3\"; message User {}"),
                    ("user.proto", "syntax = \"proto3\"; message User {}")
                ],
                limits
            )
            .is_err()
        );
    }
}
