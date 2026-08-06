//! The public, bounded JSON owner.
//!
//! The older `json` kernel is kept private to the hosted bridge.  This module
//! is the source-level implementation used by the public owner: numbers keep
//! their decimal token, parsing and streaming use explicit frames, and typed
//! paths consume the common serialization event protocol.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::io::Read;

use crate::serialization::{self, Deserialize, Event, SerializationError, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonMember {
    pub key: String,
    pub value: JsonValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<JsonMember>),
}

/// Compatibility name for code that used the provisional kernel.
pub type Value = JsonValue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonNumber {
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct JsonPath(Vec<JsonPathSegment>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonLocation {
    pub offset: usize,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonErrorKind {
    InvalidUtf8,
    InvalidSyntax,
    UnexpectedEof,
    InvalidEscape,
    InvalidUnicodeScalar,
    InvalidNumber,
    DuplicateKey,
    UnknownField,
    MissingField,
    TypeMismatch,
    NumberRange,
    LimitExceeded,
    IoError,
    TrailingData,
    CanonicalizationError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub kind: JsonErrorKind,
    pub location: JsonLocation,
    pub path: JsonPath,
}

impl JsonError {
    fn new(kind: JsonErrorKind, input: &[u8], offset: usize, path: JsonPath) -> Self {
        let offset = offset.min(input.len());
        let mut line = 1;
        let mut column = 1;
        for byte in &input[..offset] {
            if *byte == b'\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        Self {
            kind,
            location: JsonLocation {
                offset,
                line,
                column,
            },
            path,
        }
    }

    fn at_zero(kind: JsonErrorKind) -> Self {
        Self {
            kind,
            location: JsonLocation {
                offset: 0,
                line: 1,
                column: 1,
            },
            path: JsonPath::default(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "JSON {:?} at {}:{}",
            self.kind, self.location.line, self.location.column
        )
    }
}

impl std::error::Error for JsonError {}

impl fmt::Display for JsonPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("$")?;
        for segment in &self.0 {
            match segment {
                JsonPathSegment::Key(key) => write!(formatter, "[{:?}]", key)?,
                JsonPathSegment::Index(index) => write!(formatter, "[{index}]")?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonDuplicatePolicy {
    Reject,
    First,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonUnknownFieldPolicy {
    Reject,
    Ignore,
    Capture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonNumberPolicy {
    Exact,
    Float32,
    Float64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonLimits {
    pub max_document_bytes: usize,
    pub max_depth: usize,
    pub max_array_items: usize,
    pub max_object_members: usize,
    pub max_string_bytes: usize,
    pub max_number_bytes: usize,
    pub max_events: usize,
    pub max_output_bytes: usize,
}

impl Default for JsonLimits {
    fn default() -> Self {
        Self {
            max_document_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_array_items: 1_048_576,
            max_object_members: 1_048_576,
            max_string_bytes: 64 * 1024 * 1024,
            max_number_bytes: 4096,
            max_events: 1_000_000,
            max_output_bytes: 64 * 1024 * 1024,
        }
    }
}

impl JsonLimits {
    fn valid(self) -> bool {
        self.max_document_bytes > 0
            && self.max_depth > 0
            && self.max_array_items > 0
            && self.max_object_members > 0
            && self.max_string_bytes > 0
            && self.max_number_bytes > 0
            && self.max_events > 0
            && self.max_output_bytes > 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonDecodeOptions {
    pub limits: JsonLimits,
    pub duplicate_keys: JsonDuplicatePolicy,
    pub unknown_fields: JsonUnknownFieldPolicy,
    pub numbers: JsonNumberPolicy,
}

impl Default for JsonDecodeOptions {
    fn default() -> Self {
        Self {
            limits: JsonLimits::default(),
            duplicate_keys: JsonDuplicatePolicy::Reject,
            unknown_fields: JsonUnknownFieldPolicy::Reject,
            numbers: JsonNumberPolicy::Exact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JsonEncodeOptions {
    pub limits: JsonLimits,
    pub canonical: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonEvent {
    StartArray(Option<usize>),
    EndArray,
    StartObject(Option<usize>),
    EndObject,
    Key(String),
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
}

impl JsonValue {
    pub fn kind(&self) -> JsonKind {
        match self {
            Self::Null => JsonKind::Null,
            Self::Bool(_) => JsonKind::Bool,
            Self::Number(_) => JsonKind::Number,
            Self::String(_) => JsonKind::String,
            Self::Array(_) => JsonKind::Array,
            Self::Object(_) => JsonKind::Object,
        }
    }
}

impl JsonPath {
    pub fn root() -> Self {
        Self::default()
    }

    pub fn segments(&self) -> &[JsonPathSegment] {
        &self.0
    }
}

impl JsonNumber {
    pub fn parse(text: &str) -> Result<Self, JsonError> {
        if valid_number(text.as_bytes()) {
            Ok(Self {
                token: text.to_owned(),
            })
        } else {
            Err(JsonError::at_zero(JsonErrorKind::InvalidNumber))
        }
    }

    pub fn text(&self) -> String {
        self.token.clone()
    }

    pub fn to_int(&self) -> Result<i64, JsonError> {
        let (negative, digits, scale) = decimal_parts(&self.token)?;
        let mut digits = digits;
        if scale < 0 {
            let trim = usize::try_from(-scale)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
            if trim > digits.len()
                || digits[digits.len() - trim..]
                    .iter()
                    .any(|byte| *byte != b'0')
            {
                return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
            }
            digits.truncate(digits.len() - trim);
        } else if scale > 0 {
            let add = usize::try_from(scale)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
            if digits.len().checked_add(add).is_none_or(|len| len > 39) {
                return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
            }
            digits.extend(std::iter::repeat_n(b'0', add));
        }
        let magnitude = parse_u128_digits(&digits)?;
        let limit = if negative {
            (i64::MAX as u128) + 1
        } else {
            i64::MAX as u128
        };
        if magnitude > limit {
            return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
        }
        if negative {
            if magnitude == (i64::MAX as u128) + 1 {
                Ok(i64::MIN)
            } else {
                Ok(-(magnitude as i64))
            }
        } else {
            Ok(magnitude as i64)
        }
    }

    pub fn to_uint(&self) -> Result<u64, JsonError> {
        let (negative, mut digits, scale) = decimal_parts(&self.token)?;
        if negative {
            return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
        }
        if scale < 0 {
            let trim = usize::try_from(-scale)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
            if trim > digits.len()
                || digits[digits.len() - trim..]
                    .iter()
                    .any(|byte| *byte != b'0')
            {
                return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
            }
            digits.truncate(digits.len() - trim);
        } else if scale > 0 {
            let add = usize::try_from(scale)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
            if digits.len().checked_add(add).is_none_or(|len| len > 39) {
                return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
            }
            digits.extend(std::iter::repeat_n(b'0', add));
        }
        let magnitude = parse_u128_digits(&digits)?;
        u64::try_from(magnitude).map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))
    }

    pub fn to_float32(&self) -> Result<f32, JsonError> {
        let value = self
            .token
            .parse::<f32>()
            .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
        let nonzero = self
            .token
            .chars()
            .any(|character| character.is_ascii_digit() && character != '0');
        if !value.is_finite() || (value == 0.0 && nonzero) {
            return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
        }
        Ok(value)
    }

    pub fn to_float64(&self) -> Result<f64, JsonError> {
        let value = self
            .token
            .parse::<f64>()
            .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
        if !value.is_finite()
            || (value == 0.0 && self.token.chars().any(|c| c.is_ascii_digit() && c != '0'))
        {
            return Err(JsonError::at_zero(JsonErrorKind::NumberRange));
        }
        Ok(value)
    }
}

fn parse_u128_digits(digits: &[u8]) -> Result<u128, JsonError> {
    let first = digits
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(digits.len());
    digits[first..]
        .iter()
        .try_fold(0u128, |value, byte| {
            value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u128::from(byte - b'0')))
        })
        .ok_or_else(|| JsonError::at_zero(JsonErrorKind::NumberRange))
}

fn decimal_parts(text: &str) -> Result<(bool, Vec<u8>, i32), JsonError> {
    let bytes = text.as_bytes();
    let negative = bytes.first() == Some(&b'-');
    let mut index = usize::from(negative);
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let mut digits = bytes[integer_start..index].to_vec();
    let mut fraction = 0i32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        fraction = i32::try_from(index - start)
            .map_err(|_| JsonError::at_zero(JsonErrorKind::NumberRange))?;
        digits.extend_from_slice(&bytes[start..index]);
    }
    let mut exponent = 0i32;
    if bytes
        .get(index)
        .is_some_and(|byte| *byte == b'e' || *byte == b'E')
    {
        index += 1;
        let sign = if bytes.get(index) == Some(&b'-') {
            index += 1;
            -1
        } else {
            if bytes.get(index) == Some(&b'+') {
                index += 1;
            }
            1
        };
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        let parsed = std::str::from_utf8(&bytes[start..index])
            .ok()
            .and_then(|text| text.parse::<i32>().ok())
            .ok_or_else(|| JsonError::at_zero(JsonErrorKind::NumberRange))?;
        exponent = sign * parsed;
    }
    if index != bytes.len() {
        return Err(JsonError::at_zero(JsonErrorKind::InvalidNumber));
    }
    Ok((negative, digits, exponent - fraction))
}

fn valid_number(bytes: &[u8]) -> bool {
    let mut index = 0;
    if bytes.get(index) == Some(&b'-') {
        index += 1;
    }
    match bytes.get(index) {
        Some(b'0') => index += 1,
        Some(b'1'..=b'9') => {
            index += 1;
            while bytes.get(index).is_some_and(u8::is_ascii_digit) {
                index += 1;
            }
        }
        _ => return false,
    }
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if start == index {
            return false;
        }
    }
    if bytes
        .get(index)
        .is_some_and(|byte| *byte == b'e' || *byte == b'E')
    {
        index += 1;
        if bytes
            .get(index)
            .is_some_and(|byte| *byte == b'+' || *byte == b'-')
        {
            index += 1;
        }
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if start == index {
            return false;
        }
    }
    index == bytes.len()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayState {
    FirstOrValue,
    ValueAfterComma,
    CommaOrEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectState {
    FirstOrKey,
    KeyAfterComma,
    Colon,
    Value,
    CommaOrEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Frame {
    Array {
        state: ArrayState,
        items: usize,
    },
    Object {
        state: ObjectState,
        members: usize,
        keys: Vec<String>,
    },
}

pub struct JsonReader<'a> {
    input: Cow<'a, [u8]>,
    cursor: usize,
    options: JsonDecodeOptions,
    stack: Vec<Frame>,
    root_complete: bool,
    eof_returned: bool,
    events: usize,
    terminal: Option<JsonError>,
}

impl<'a> JsonReader<'a> {
    pub fn from_bytes(input: &'a [u8], options: JsonDecodeOptions) -> Result<Self, JsonError> {
        Self::new(Cow::Borrowed(input), options)
    }

    pub fn from_reader<R: Read>(
        mut input: R,
        options: JsonDecodeOptions,
    ) -> Result<JsonReader<'static>, JsonError> {
        if !options.limits.valid() {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        let mut bytes = Vec::new();
        let limit = options.limits.max_document_bytes;
        let mut chunk = [0u8; 8192];
        while bytes.len() <= limit {
            let read = input
                .read(&mut chunk)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::IoError))?;
            if read == 0 {
                break;
            }
            if bytes.len().checked_add(read).is_none_or(|len| len > limit) {
                return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        JsonReader::<'static>::new(Cow::Owned(bytes), options)
    }

    pub fn from_chunks<'b, I>(
        chunks: I,
        options: JsonDecodeOptions,
    ) -> Result<JsonReader<'static>, JsonError>
    where
        I: IntoIterator<Item = &'b [u8]>,
    {
        if !options.limits.valid() {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        let mut bytes = Vec::new();
        for chunk in chunks {
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|len| len > options.limits.max_document_bytes)
            {
                return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
            }
            bytes.extend_from_slice(chunk);
        }
        JsonReader::<'static>::new(Cow::Owned(bytes), options)
    }

    fn new(input: Cow<'a, [u8]>, options: JsonDecodeOptions) -> Result<Self, JsonError> {
        if !options.limits.valid() {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        if input.len() > options.limits.max_document_bytes {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        Ok(Self {
            input,
            cursor: 0,
            options,
            stack: Vec::new(),
            root_complete: false,
            eof_returned: false,
            events: 0,
            terminal: None,
        })
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<JsonEvent>, JsonError> {
        let result = self.next_inner();
        if let Err(error) = &result {
            self.terminal = Some(error.clone());
        }
        result
    }

    fn next_inner(&mut self) -> Result<Option<JsonEvent>, JsonError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        loop {
            if self.stack.is_empty() {
                if self.root_complete {
                    self.skip_whitespace();
                    if self.cursor != self.input.len() {
                        return self.fail(JsonErrorKind::TrailingData);
                    }
                    self.eof_returned = true;
                    return Ok(None);
                }
                let event = self.parse_value()?;
                if !matches!(event, JsonEvent::StartArray(_) | JsonEvent::StartObject(_)) {
                    self.root_complete = true;
                }
                return self.publish(event).map(Some);
            }

            let state = self.stack.last().expect("non-empty frame").clone();
            match state {
                Frame::Array {
                    state: ArrayState::FirstOrValue,
                    ..
                } => {
                    self.skip_whitespace();
                    if self.peek().is_none() {
                        return self.fail(JsonErrorKind::UnexpectedEof);
                    }
                    if self.peek() == Some(b']') {
                        self.cursor += 1;
                        self.stack.pop();
                        self.parent_closed();
                        return self.publish(JsonEvent::EndArray).map(Some);
                    }
                    if self.array_items() >= self.options.limits.max_array_items {
                        return self.fail(JsonErrorKind::LimitExceeded);
                    }
                    self.set_array_state(ArrayState::CommaOrEnd);
                    let event = self.parse_value()?;
                    return self.publish(event).map(Some);
                }
                Frame::Array {
                    state: ArrayState::ValueAfterComma,
                    ..
                } => {
                    self.skip_whitespace();
                    if self.peek().is_none() {
                        return self.fail(JsonErrorKind::UnexpectedEof);
                    }
                    if self.peek() == Some(b']') {
                        return self.fail(JsonErrorKind::InvalidSyntax);
                    }
                    if self.array_items() >= self.options.limits.max_array_items {
                        return self.fail(JsonErrorKind::LimitExceeded);
                    }
                    self.set_array_state(ArrayState::CommaOrEnd);
                    let event = self.parse_value()?;
                    return self.publish(event).map(Some);
                }
                Frame::Array {
                    state: ArrayState::CommaOrEnd,
                    ..
                } => {
                    self.skip_whitespace();
                    match self.peek() {
                        Some(b',') => {
                            self.cursor += 1;
                            self.set_array_state(ArrayState::ValueAfterComma);
                        }
                        Some(b']') => {
                            self.cursor += 1;
                            self.stack.pop();
                            self.parent_closed();
                            return self.publish(JsonEvent::EndArray).map(Some);
                        }
                        _ => return self.fail(JsonErrorKind::InvalidSyntax),
                    }
                }
                Frame::Object {
                    state: ObjectState::FirstOrKey,
                    ..
                } => {
                    self.skip_whitespace();
                    if self.peek().is_none() {
                        return self.fail(JsonErrorKind::UnexpectedEof);
                    }
                    if self.peek() == Some(b'}') {
                        self.cursor += 1;
                        self.stack.pop();
                        self.parent_closed();
                        return self.publish(JsonEvent::EndObject).map(Some);
                    }
                    return self.parse_key();
                }
                Frame::Object {
                    state: ObjectState::KeyAfterComma,
                    ..
                } => {
                    self.skip_whitespace();
                    if self.peek().is_none() {
                        return self.fail(JsonErrorKind::UnexpectedEof);
                    }
                    if self.peek() == Some(b'}') {
                        return self.fail(JsonErrorKind::InvalidSyntax);
                    }
                    return self.parse_key();
                }
                Frame::Object {
                    state: ObjectState::Colon,
                    ..
                } => {
                    self.skip_whitespace();
                    if self.peek() != Some(b':') {
                        return self.fail(JsonErrorKind::InvalidSyntax);
                    }
                    self.cursor += 1;
                    self.set_object_state(ObjectState::Value);
                }
                Frame::Object {
                    state: ObjectState::Value,
                    ..
                } => {
                    self.set_object_state(ObjectState::CommaOrEnd);
                    let event = self.parse_value()?;
                    return self.publish(event).map(Some);
                }
                Frame::Object {
                    state: ObjectState::CommaOrEnd,
                    ..
                } => {
                    self.skip_whitespace();
                    match self.peek() {
                        Some(b',') => {
                            self.cursor += 1;
                            self.set_object_state(ObjectState::KeyAfterComma);
                        }
                        Some(b'}') => {
                            self.cursor += 1;
                            self.stack.pop();
                            self.parent_closed();
                            return self.publish(JsonEvent::EndObject).map(Some);
                        }
                        _ => return self.fail(JsonErrorKind::InvalidSyntax),
                    }
                }
            }
        }
    }

    pub fn own(&mut self, event: JsonEvent) -> Result<JsonEvent, JsonError> {
        Ok(event)
    }

    pub fn finish(&mut self) -> Result<(), JsonError> {
        while self.next()?.is_some() {}
        if self.root_complete && self.stack.is_empty() {
            Ok(())
        } else {
            self.fail(JsonErrorKind::UnexpectedEof)
        }
    }

    fn parse_key(&mut self) -> Result<Option<JsonEvent>, JsonError> {
        let key = self.parse_string()?;
        if let Some(Frame::Object {
            members,
            keys,
            state,
            ..
        }) = self.stack.last_mut()
        {
            if *members >= self.options.limits.max_object_members {
                return self.fail(JsonErrorKind::LimitExceeded);
            }
            if keys.iter().any(|candidate| candidate == &key)
                && self.options.duplicate_keys == JsonDuplicatePolicy::Reject
            {
                return self.fail(JsonErrorKind::DuplicateKey);
            }
            *members += 1;
            keys.push(key.clone());
            *state = ObjectState::Colon;
        }
        self.publish(JsonEvent::Key(key)).map(Some)
    }

    fn parse_value(&mut self) -> Result<JsonEvent, JsonError> {
        self.skip_whitespace();
        if self.peek().is_none() {
            return self.fail(JsonErrorKind::UnexpectedEof);
        }
        match self.peek() {
            Some(b'[') => {
                self.cursor += 1;
                if self.stack.len() >= self.options.limits.max_depth {
                    return self.fail(JsonErrorKind::LimitExceeded);
                }
                self.stack.push(Frame::Array {
                    state: ArrayState::FirstOrValue,
                    items: 0,
                });
                Ok(JsonEvent::StartArray(None))
            }
            Some(b'{') => {
                self.cursor += 1;
                if self.stack.len() >= self.options.limits.max_depth {
                    return self.fail(JsonErrorKind::LimitExceeded);
                }
                self.stack.push(Frame::Object {
                    state: ObjectState::FirstOrKey,
                    members: 0,
                    keys: Vec::new(),
                });
                Ok(JsonEvent::StartObject(None))
            }
            Some(b'"') => Ok(JsonEvent::String(self.parse_string()?)),
            Some(b't') => {
                self.literal(b"true")?;
                Ok(JsonEvent::Bool(true))
            }
            Some(b'f') => {
                self.literal(b"false")?;
                Ok(JsonEvent::Bool(false))
            }
            Some(b'n') => {
                self.literal(b"null")?;
                Ok(JsonEvent::Null)
            }
            Some(b'-' | b'0'..=b'9') => Ok(JsonEvent::Number(self.parse_number()?)),
            _ => self.fail(JsonErrorKind::InvalidSyntax),
        }
    }

    fn parse_number(&mut self) -> Result<JsonNumber, JsonError> {
        let start = self.cursor;
        while let Some(byte) = self.peek() {
            if matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | b',' | b']' | b'}') {
                break;
            }
            self.cursor += 1;
        }
        if self.cursor - start > self.options.limits.max_number_bytes {
            return self.fail(JsonErrorKind::LimitExceeded);
        }
        let token = std::str::from_utf8(&self.input[start..self.cursor]).map_err(|_| {
            JsonError::new(
                JsonErrorKind::InvalidUtf8,
                &self.input,
                start,
                self.current_path(),
            )
        })?;
        JsonNumber::parse(token).map_err(|_| {
            JsonError::new(
                JsonErrorKind::InvalidNumber,
                &self.input,
                start,
                self.current_path(),
            )
        })
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        if self.peek() != Some(b'"') {
            return self.fail(JsonErrorKind::InvalidSyntax);
        }
        self.cursor += 1;
        let mut output = Vec::new();
        while let Some(byte) = self.peek() {
            self.cursor += 1;
            match byte {
                b'"' => {
                    if output.len() > self.options.limits.max_string_bytes {
                        return self.fail(JsonErrorKind::LimitExceeded);
                    }
                    return String::from_utf8(output).map_err(|_| {
                        JsonError::new(
                            JsonErrorKind::InvalidUtf8,
                            &self.input,
                            self.cursor,
                            self.current_path(),
                        )
                    });
                }
                b'\\' => {
                    let escape = self.peek().ok_or_else(|| {
                        JsonError::new(
                            JsonErrorKind::UnexpectedEof,
                            &self.input,
                            self.cursor,
                            self.current_path(),
                        )
                    })?;
                    self.cursor += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => output.push(escape),
                        b'b' => output.push(8),
                        b'f' => output.push(12),
                        b'n' => output.push(b'\n'),
                        b'r' => output.push(b'\r'),
                        b't' => output.push(b'\t'),
                        b'u' => self.parse_unicode_escape(&mut output)?,
                        _ => return self.fail(JsonErrorKind::InvalidEscape),
                    }
                }
                0..=0x1f => return self.fail(JsonErrorKind::InvalidSyntax),
                _ => {
                    let width = utf8_width(byte).ok_or_else(|| {
                        JsonError::new(
                            JsonErrorKind::InvalidUtf8,
                            &self.input,
                            self.cursor - 1,
                            self.current_path(),
                        )
                    })?;
                    let end = self
                        .cursor
                        .checked_add(width - 1)
                        .ok_or_else(|| JsonError::at_zero(JsonErrorKind::LimitExceeded))?;
                    let bytes = self.input.get(self.cursor - 1..end).ok_or_else(|| {
                        JsonError::new(
                            JsonErrorKind::UnexpectedEof,
                            &self.input,
                            self.cursor,
                            self.current_path(),
                        )
                    })?;
                    if std::str::from_utf8(bytes).is_err() {
                        return self.fail(JsonErrorKind::InvalidUtf8);
                    }
                    output.extend_from_slice(bytes);
                    self.cursor = end;
                }
            }
            if output.len() > self.options.limits.max_string_bytes {
                return self.fail(JsonErrorKind::LimitExceeded);
            }
        }
        self.fail(JsonErrorKind::UnexpectedEof)
    }

    fn parse_unicode_escape(&mut self, output: &mut Vec<u8>) -> Result<(), JsonError> {
        let high = self.hex_quad()?;
        let scalar = if (0xD800..=0xDBFF).contains(&high) {
            if self.input.get(self.cursor..self.cursor + 2) != Some(b"\\u") {
                return self.fail(JsonErrorKind::InvalidUnicodeScalar);
            }
            self.cursor += 2;
            let low = self.hex_quad()?;
            if !(0xDC00..=0xDFFF).contains(&low) {
                return self.fail(JsonErrorKind::InvalidUnicodeScalar);
            }
            0x10000 + ((u32::from(high) - 0xD800) << 10) + (u32::from(low) - 0xDC00)
        } else if (0xDC00..=0xDFFF).contains(&high) {
            return self.fail(JsonErrorKind::InvalidUnicodeScalar);
        } else {
            u32::from(high)
        };
        let character = char::from_u32(scalar).ok_or_else(|| {
            JsonError::new(
                JsonErrorKind::InvalidUnicodeScalar,
                &self.input,
                self.cursor,
                self.current_path(),
            )
        })?;
        let mut encoded = [0u8; 4];
        output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
        Ok(())
    }

    fn hex_quad(&mut self) -> Result<u16, JsonError> {
        let end = self
            .cursor
            .checked_add(4)
            .ok_or_else(|| JsonError::at_zero(JsonErrorKind::LimitExceeded))?;
        let bytes = self.input.get(self.cursor..end).ok_or_else(|| {
            JsonError::new(
                JsonErrorKind::UnexpectedEof,
                &self.input,
                self.cursor,
                self.current_path(),
            )
        })?;
        self.cursor = end;
        bytes
            .iter()
            .try_fold(0u16, |value, byte| {
                hex_value(*byte).map(|digit| (value << 4) | u16::from(digit))
            })
            .ok_or_else(|| {
                JsonError::new(
                    JsonErrorKind::InvalidEscape,
                    &self.input,
                    self.cursor.saturating_sub(4),
                    self.current_path(),
                )
            })
    }

    fn literal(&mut self, literal: &[u8]) -> Result<(), JsonError> {
        let end = self
            .cursor
            .checked_add(literal.len())
            .ok_or_else(|| JsonError::at_zero(JsonErrorKind::LimitExceeded))?;
        if self.input.get(self.cursor..end) != Some(literal) {
            return self.fail(JsonErrorKind::InvalidSyntax);
        }
        self.cursor = end;
        Ok(())
    }

    fn publish(&mut self, event: JsonEvent) -> Result<JsonEvent, JsonError> {
        if self.events >= self.options.limits.max_events {
            return self.fail(JsonErrorKind::LimitExceeded);
        }
        self.events += 1;
        Ok(event)
    }

    fn parent_closed(&mut self) {
        if self.stack.is_empty() {
            self.root_complete = true;
        }
    }

    fn set_array_state(&mut self, state: ArrayState) {
        if let Some(Frame::Array {
            state: current,
            items,
        }) = self.stack.last_mut()
        {
            *current = state;
            if state == ArrayState::CommaOrEnd {
                *items += 1;
            }
        }
    }

    fn array_items(&self) -> usize {
        match self.stack.last() {
            Some(Frame::Array { items, .. }) => *items,
            _ => 0,
        }
    }

    fn set_object_state(&mut self, state: ObjectState) {
        if let Some(Frame::Object { state: current, .. }) = self.stack.last_mut() {
            *current = state;
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.cursor += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.cursor).copied()
    }

    fn fail<T>(&mut self, kind: JsonErrorKind) -> Result<T, JsonError> {
        let error = JsonError::new(kind, &self.input, self.cursor, self.current_path());
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn current_path(&self) -> JsonPath {
        let mut path = JsonPath::root();
        for frame in &self.stack {
            match frame {
                Frame::Array { state, items } => {
                    let index = if *state == ArrayState::CommaOrEnd {
                        items.saturating_sub(1)
                    } else {
                        *items
                    };
                    path.0.push(JsonPathSegment::Index(index));
                }
                Frame::Object { state, keys, .. } => {
                    if (*state == ObjectState::Value || *state == ObjectState::CommaOrEnd)
                        && let Some(key) = keys.last()
                    {
                        path.0.push(JsonPathSegment::Key(key.clone()));
                    }
                }
            }
        }
        path
    }
}

fn utf8_width(first: u8) -> Option<usize> {
    match first {
        0..=0x7f => Some(1),
        0xc2..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf4 => Some(4),
        _ => None,
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn parse(input: &[u8]) -> Result<JsonValue, JsonError> {
    parse_with_options(input, JsonDecodeOptions::default())
}

pub fn parse_with_options(
    input: &[u8],
    options: JsonDecodeOptions,
) -> Result<JsonValue, JsonError> {
    let duplicate_policy = options.duplicate_keys;
    let mut reader = JsonReader::from_bytes(input, options)?;
    let mut frames = Vec::new();
    let mut root = None;
    while let Some(event) = reader.next()? {
        match event {
            JsonEvent::StartArray(_) => frames.push(BuildFrame::Array(Vec::new())),
            JsonEvent::StartObject(_) => frames.push(BuildFrame::Object {
                members: Vec::new(),
                pending_key: None,
            }),
            JsonEvent::EndArray => {
                let Some(BuildFrame::Array(values)) = frames.pop() else {
                    return Err(JsonError::at_zero(JsonErrorKind::InvalidSyntax));
                };
                attach_value(
                    JsonValue::Array(values),
                    &mut frames,
                    &mut root,
                    duplicate_policy,
                )?;
            }
            JsonEvent::EndObject => {
                let Some(BuildFrame::Object {
                    members,
                    pending_key,
                }) = frames.pop()
                else {
                    return Err(JsonError::at_zero(JsonErrorKind::InvalidSyntax));
                };
                if pending_key.is_some() {
                    return Err(JsonError::at_zero(JsonErrorKind::InvalidSyntax));
                }
                attach_value(
                    JsonValue::Object(members),
                    &mut frames,
                    &mut root,
                    duplicate_policy,
                )?;
            }
            JsonEvent::Key(key) => match frames.last_mut() {
                Some(BuildFrame::Object { pending_key, .. }) if pending_key.is_none() => {
                    *pending_key = Some(key)
                }
                _ => return Err(JsonError::at_zero(JsonErrorKind::InvalidSyntax)),
            },
            JsonEvent::Null => {
                attach_value(JsonValue::Null, &mut frames, &mut root, duplicate_policy)?
            }
            JsonEvent::Bool(value) => attach_value(
                JsonValue::Bool(value),
                &mut frames,
                &mut root,
                duplicate_policy,
            )?,
            JsonEvent::Number(value) => attach_value(
                JsonValue::Number(value),
                &mut frames,
                &mut root,
                duplicate_policy,
            )?,
            JsonEvent::String(value) => attach_value(
                JsonValue::String(value),
                &mut frames,
                &mut root,
                duplicate_policy,
            )?,
        }
    }
    reader.finish()?;
    if !frames.is_empty() {
        return Err(JsonError::at_zero(JsonErrorKind::UnexpectedEof));
    }
    root.ok_or_else(|| JsonError::at_zero(JsonErrorKind::UnexpectedEof))
}

enum BuildFrame {
    Array(Vec<JsonValue>),
    Object {
        members: Vec<JsonMember>,
        pending_key: Option<String>,
    },
}

fn attach_value(
    value: JsonValue,
    frames: &mut [BuildFrame],
    root: &mut Option<JsonValue>,
    duplicate_policy: JsonDuplicatePolicy,
) -> Result<(), JsonError> {
    match frames.last_mut() {
        Some(BuildFrame::Array(values)) => values.push(value),
        Some(BuildFrame::Object {
            members,
            pending_key,
        }) => {
            let Some(key) = pending_key.take() else {
                return Err(JsonError::at_zero(JsonErrorKind::InvalidSyntax));
            };
            if let Some(position) = members.iter().position(|member| member.key == key) {
                match duplicate_policy {
                    JsonDuplicatePolicy::Reject => {
                        return Err(JsonError::at_zero(JsonErrorKind::DuplicateKey));
                    }
                    JsonDuplicatePolicy::First => return Ok(()),
                    JsonDuplicatePolicy::Last => members[position].value = value,
                }
            } else {
                members.push(JsonMember { key, value });
            }
        }
        None => {
            if root.is_some() {
                return Err(JsonError::at_zero(JsonErrorKind::TrailingData));
            }
            *root = Some(value);
        }
    }
    Ok(())
}

pub fn validate(input: &[u8]) -> Result<(), JsonError> {
    validate_with_options(input, JsonDecodeOptions::default())
}

pub fn validate_with_options(input: &[u8], options: JsonDecodeOptions) -> Result<(), JsonError> {
    let mut reader = JsonReader::from_bytes(input, options)?;
    reader.finish()
}

pub fn canonicalize(input: &[u8]) -> Result<Vec<u8>, JsonError> {
    canonicalize_with_options(input, JsonDecodeOptions::default())
}

pub fn canonicalize_with_options(
    input: &[u8],
    options: JsonDecodeOptions,
) -> Result<Vec<u8>, JsonError> {
    let value = parse_with_options(input, options)?;
    encode_with_options(
        &value,
        JsonEncodeOptions {
            canonical: true,
            limits: options.limits,
        },
    )
}

pub fn encode(value: &JsonValue) -> Result<Vec<u8>, JsonError> {
    encode_with_options(value, JsonEncodeOptions::default())
}

pub fn encode_canonical(value: &JsonValue) -> Result<Vec<u8>, JsonError> {
    encode_with_options(
        value,
        JsonEncodeOptions {
            canonical: true,
            ..JsonEncodeOptions::default()
        },
    )
}

pub fn encode_canonical_with_limits(
    value: &JsonValue,
    limits: JsonLimits,
) -> Result<Vec<u8>, JsonError> {
    encode_with_options(
        value,
        JsonEncodeOptions {
            canonical: true,
            limits,
        },
    )
}

pub fn encode_with_options(
    value: &JsonValue,
    options: JsonEncodeOptions,
) -> Result<Vec<u8>, JsonError> {
    let mut output = JsonOutput::new(options.limits)?;
    let mut tasks = vec![EncodeTask::Value(value)];
    while let Some(task) = tasks.pop() {
        match task {
            EncodeTask::Value(value) => match value {
                JsonValue::Null => output.bytes(b"null")?,
                JsonValue::Bool(value) => output.bytes(if *value { b"true" } else { b"false" })?,
                JsonValue::Number(number) => output.text(if options.canonical {
                    canonical_number(number)?
                } else {
                    number.token.clone()
                })?,
                JsonValue::String(value) => write_string(&mut output, value)?,
                JsonValue::Array(values) => {
                    output.bytes(b"[")?;
                    tasks.push(EncodeTask::Bytes(b"]"));
                    for (index, value) in values.iter().enumerate().rev() {
                        if index + 1 < values.len() {
                            tasks.push(EncodeTask::Bytes(b","));
                        }
                        tasks.push(EncodeTask::Value(value));
                    }
                }
                JsonValue::Object(members) => {
                    let order = member_order(members, options.canonical)?;
                    output.bytes(b"{")?;
                    tasks.push(EncodeTask::Bytes(b"}"));
                    for (position, index) in order.iter().enumerate().rev() {
                        if position + 1 < order.len() {
                            tasks.push(EncodeTask::Bytes(b","));
                        }
                        tasks.push(EncodeTask::Member(&members[*index]));
                    }
                }
            },
            EncodeTask::Member(member) => {
                write_string(&mut output, &member.key)?;
                output.bytes(b":")?;
                tasks.push(EncodeTask::Value(&member.value));
            }
            EncodeTask::Bytes(bytes) => output.bytes(bytes)?,
        }
    }
    Ok(output.finish())
}

enum EncodeTask<'a> {
    Value(&'a JsonValue),
    Member(&'a JsonMember),
    Bytes(&'static [u8]),
}

struct JsonOutput {
    bytes: Vec<u8>,
    limits: JsonLimits,
}

impl JsonOutput {
    fn new(limits: JsonLimits) -> Result<Self, JsonError> {
        if !limits.valid() {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        Ok(Self {
            bytes: Vec::new(),
            limits,
        })
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<(), JsonError> {
        if self
            .bytes
            .len()
            .checked_add(bytes.len())
            .is_none_or(|len| len > self.limits.max_output_bytes)
        {
            return Err(JsonError::at_zero(JsonErrorKind::LimitExceeded));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn text(&mut self, text: String) -> Result<(), JsonError> {
        self.bytes(text.as_bytes())
    }
    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

fn member_order(members: &[JsonMember], canonical: bool) -> Result<Vec<usize>, JsonError> {
    let mut order = (0..members.len()).collect::<Vec<_>>();
    if !canonical {
        return Ok(order);
    }
    order.sort_by(|left, right| {
        utf16_key(&members[*left].key).cmp(&utf16_key(&members[*right].key))
    });
    if order
        .windows(2)
        .any(|pair| members[pair[0]].key == members[pair[1]].key)
    {
        return Err(JsonError::at_zero(JsonErrorKind::CanonicalizationError));
    }
    Ok(order)
}

fn utf16_key(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

fn canonical_number(number: &JsonNumber) -> Result<String, JsonError> {
    let token = number.token.as_str();
    let (negative, mut digits, mut scale) = decimal_parts(token)?;
    while digits.len() > 1 && digits.last() == Some(&b'0') && scale < 0 {
        digits.pop();
        scale += 1;
    }
    let first = digits
        .iter()
        .position(|byte| *byte != b'0')
        .unwrap_or(digits.len());
    if first == digits.len() {
        return Ok("0".into());
    }
    digits = digits[first..].to_vec();
    let decimal = i32::try_from(digits.len()).unwrap_or(i32::MAX) + scale;
    let mut result = String::new();
    if negative {
        result.push('-');
    }
    if decimal > 0 && decimal <= i32::try_from(digits.len()).unwrap_or(i32::MAX) {
        let split = usize::try_from(decimal)
            .map_err(|_| JsonError::at_zero(JsonErrorKind::CanonicalizationError))?;
        result.push_str(
            std::str::from_utf8(&digits[..split])
                .map_err(|_| JsonError::at_zero(JsonErrorKind::CanonicalizationError))?,
        );
        if split < digits.len() {
            result.push('.');
            result.push_str(
                std::str::from_utf8(&digits[split..])
                    .map_err(|_| JsonError::at_zero(JsonErrorKind::CanonicalizationError))?,
            );
        }
    } else if decimal <= 0 {
        result.push_str("0.");
        result.extend(std::iter::repeat_n(
            '0',
            usize::try_from(-decimal).unwrap_or(usize::MAX).min(4096),
        ));
        result.push_str(
            std::str::from_utf8(&digits)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::CanonicalizationError))?,
        );
    } else {
        result.push_str(
            std::str::from_utf8(&digits)
                .map_err(|_| JsonError::at_zero(JsonErrorKind::CanonicalizationError))?,
        );
        result.extend(std::iter::repeat_n(
            '0',
            usize::try_from(decimal - i32::try_from(digits.len()).unwrap_or(i32::MAX))
                .unwrap_or(usize::MAX)
                .min(4096),
        ));
    }
    Ok(result)
}

fn write_string(output: &mut JsonOutput, value: &str) -> Result<(), JsonError> {
    output.bytes(b"\"")?;
    for character in value.chars() {
        match character {
            '"' => output.bytes(b"\\\"")?,
            '\\' => output.bytes(b"\\\\")?,
            '\u{08}' => output.bytes(b"\\b")?,
            '\u{0c}' => output.bytes(b"\\f")?,
            '\n' => output.bytes(b"\\n")?,
            '\r' => output.bytes(b"\\r")?,
            '\t' => output.bytes(b"\\t")?,
            c if c.is_control() => {
                let escape = format!("\\u{:04x}", c as u32);
                output.text(escape)?;
            }
            c => {
                let mut encoded = [0u8; 4];
                output.bytes(c.encode_utf8(&mut encoded).as_bytes())?;
            }
        }
    }
    output.bytes(b"\"")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriterArrayState {
    First,
    CommaOrEnd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WriterFrame {
    Array(WriterArrayState),
    Object {
        state: ObjectState,
        keys: Vec<String>,
    },
}

pub struct JsonWriter {
    output: JsonOutput,
    stack: Vec<WriterFrame>,
    root_written: bool,
    finished: bool,
    terminal: Option<JsonError>,
    canonical: bool,
}

impl JsonWriter {
    pub fn to_writer(options: JsonEncodeOptions) -> Result<Self, JsonError> {
        Ok(Self {
            output: JsonOutput::new(options.limits)?,
            stack: Vec::new(),
            root_written: false,
            finished: false,
            terminal: None,
            canonical: options.canonical,
        })
    }

    pub fn write(&mut self, event: JsonEvent) -> Result<(), JsonError> {
        let result = self.write_inner(event);
        if let Err(error) = &result {
            self.terminal = Some(error.clone());
        }
        result
    }

    fn write_inner(&mut self, event: JsonEvent) -> Result<(), JsonError> {
        if self.finished {
            return self.fail(JsonErrorKind::InvalidSyntax);
        }
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        match event {
            JsonEvent::StartArray(_) => {
                self.before_value()?;
                self.output.bytes(b"[")?;
                self.stack.push(WriterFrame::Array(WriterArrayState::First));
            }
            JsonEvent::EndArray => {
                if !matches!(
                    self.stack.last(),
                    Some(WriterFrame::Array(
                        WriterArrayState::First | WriterArrayState::CommaOrEnd
                    ))
                ) {
                    return self.fail(JsonErrorKind::InvalidSyntax);
                }
                self.output.bytes(b"]")?;
                self.stack.pop();
            }
            JsonEvent::StartObject(_) => {
                self.before_value()?;
                self.output.bytes(b"{")?;
                self.stack.push(WriterFrame::Object {
                    state: ObjectState::FirstOrKey,
                    keys: Vec::new(),
                });
            }
            JsonEvent::EndObject => {
                if !matches!(
                    self.stack.last(),
                    Some(WriterFrame::Object {
                        state: ObjectState::FirstOrKey | ObjectState::CommaOrEnd,
                        ..
                    })
                ) {
                    return self.fail(JsonErrorKind::InvalidSyntax);
                }
                self.output.bytes(b"}")?;
                self.stack.pop();
            }
            JsonEvent::Key(key) => {
                let Some(WriterFrame::Object { state, keys }) = self.stack.last_mut() else {
                    return self.fail(JsonErrorKind::InvalidSyntax);
                };
                if !matches!(
                    *state,
                    ObjectState::FirstOrKey | ObjectState::KeyAfterComma | ObjectState::CommaOrEnd
                ) {
                    return self.fail(JsonErrorKind::InvalidSyntax);
                }
                if self.canonical
                    && keys
                        .last()
                        .is_some_and(|last| utf16_key(last).cmp(&utf16_key(&key)) != Ordering::Less)
                {
                    return self.fail(JsonErrorKind::CanonicalizationError);
                }
                if keys.iter().any(|item| item == &key) {
                    return self.fail(JsonErrorKind::DuplicateKey);
                }
                if matches!(*state, ObjectState::KeyAfterComma | ObjectState::CommaOrEnd) {
                    self.output.bytes(b",")?;
                }
                write_string(&mut self.output, &key)?;
                self.output.bytes(b":")?;
                keys.push(key);
                *state = ObjectState::Value;
            }
            JsonEvent::Null => {
                self.before_value()?;
                self.output.bytes(b"null")?;
            }
            JsonEvent::Bool(value) => {
                self.before_value()?;
                self.output.bytes(if value { b"true" } else { b"false" })?;
            }
            JsonEvent::Number(number) => {
                self.before_value()?;
                self.output.text(if self.canonical {
                    canonical_number(&number)?
                } else {
                    number.token
                })?;
            }
            JsonEvent::String(value) => {
                self.before_value()?;
                write_string(&mut self.output, &value)?;
            }
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, JsonError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return self.fail(JsonErrorKind::InvalidSyntax);
        }
        if !self.root_written || !self.stack.is_empty() {
            return self.fail(JsonErrorKind::UnexpectedEof);
        }
        self.finished = true;
        Ok(std::mem::take(&mut self.output.bytes))
    }

    fn before_value(&mut self) -> Result<(), JsonError> {
        if self.stack.is_empty() {
            if self.root_written {
                return self.fail(JsonErrorKind::TrailingData);
            }
            self.root_written = true;
            return Ok(());
        }
        match self.stack.last_mut().expect("non-empty writer frame") {
            WriterFrame::Array(state) => match state {
                WriterArrayState::First => *state = WriterArrayState::CommaOrEnd,
                WriterArrayState::CommaOrEnd => self.output.bytes(b",")?,
            },
            WriterFrame::Object { state, .. } => {
                if *state != ObjectState::Value {
                    return self.fail(JsonErrorKind::InvalidSyntax);
                }
                *state = ObjectState::CommaOrEnd;
            }
        }
        Ok(())
    }

    fn fail<T>(&mut self, kind: JsonErrorKind) -> Result<T, JsonError> {
        let error = JsonError::at_zero(kind);
        self.terminal = Some(error.clone());
        Err(error)
    }
}

pub fn encode_typed<T: Serialize>(
    value: &T,
    options: JsonEncodeOptions,
) -> Result<Vec<u8>, JsonError> {
    let limits = serialization::Limits {
        max_depth: options.limits.max_depth,
        max_events: options.limits.max_events,
        max_bytes: options.limits.max_output_bytes,
        max_container_items: options
            .limits
            .max_array_items
            .max(options.limits.max_object_members),
    };
    let events = serialization::serialize_value(value, limits).map_err(map_serialization_error)?;
    let mut writer = JsonWriter::to_writer(options)?;
    for event in events {
        writer.write(event_to_json(event)?)?;
    }
    writer.finish()
}

pub fn decode_typed<T: Deserialize>(
    input: &[u8],
    options: JsonDecodeOptions,
) -> Result<T, JsonError> {
    let mut reader = JsonReader::from_bytes(input, options)?;
    let mut events = Vec::new();
    while let Some(event) = reader.next()? {
        events.push(json_to_event(event)?);
    }
    reader.finish()?;
    let limits = serialization::Limits {
        max_depth: options.limits.max_depth,
        max_events: options.limits.max_events,
        max_bytes: options.limits.max_document_bytes,
        max_container_items: options
            .limits
            .max_array_items
            .max(options.limits.max_object_members),
    };
    serialization::deserialize_value::<T>(&events, limits).map_err(map_serialization_error)
}

fn event_to_json(event: Event) -> Result<JsonEvent, JsonError> {
    match event {
        Event::Null => Ok(JsonEvent::Null),
        Event::Bool(value) => Ok(JsonEvent::Bool(value)),
        Event::Int(value) => JsonNumber::parse(&value.to_string()).map(JsonEvent::Number),
        Event::UInt(value) => JsonNumber::parse(&value.to_string()).map(JsonEvent::Number),
        Event::Float(value) => JsonNumber::parse(&value.to_string()).map(JsonEvent::Number),
        Event::Float32(value) => {
            JsonNumber::parse(&f32::from_bits(value).to_string()).map(JsonEvent::Number)
        }
        Event::Float64(value) => {
            JsonNumber::parse(&f64::from_bits(value).to_string()).map(JsonEvent::Number)
        }
        Event::String(value) => Ok(JsonEvent::String(value)),
        Event::StartArray(length) => Ok(JsonEvent::StartArray(length)),
        Event::EndArray => Ok(JsonEvent::EndArray),
        Event::StartRecord { fields, .. } => Ok(JsonEvent::StartObject(fields)),
        Event::Field(field) => Ok(JsonEvent::Key(field)),
        Event::EndRecord => Ok(JsonEvent::EndObject),
        Event::StartMap(length) => Ok(JsonEvent::StartObject(length)),
        Event::MapKey => Err(JsonError::at_zero(JsonErrorKind::TypeMismatch)),
        Event::EndMap => Ok(JsonEvent::EndObject),
        Event::Bytes(_) | Event::StartEnum { .. } | Event::EndEnum => {
            Err(JsonError::at_zero(JsonErrorKind::TypeMismatch))
        }
    }
}

fn json_to_event(event: JsonEvent) -> Result<Event, JsonError> {
    match event {
        JsonEvent::Null => Ok(Event::Null),
        JsonEvent::Bool(value) => Ok(Event::Bool(value)),
        JsonEvent::Number(number) => {
            if number.token.contains(['.', 'e', 'E']) {
                Ok(Event::Float64(number.to_float64()?.to_bits()))
            } else if number.token.starts_with('-') {
                Ok(Event::Int(i128::from(number.to_int()?)))
            } else {
                Ok(Event::UInt(u128::from(number.to_uint()?)))
            }
        }
        JsonEvent::String(value) => Ok(Event::String(value)),
        JsonEvent::StartArray(length) => Ok(Event::StartArray(length)),
        JsonEvent::EndArray => Ok(Event::EndArray),
        JsonEvent::StartObject(length) => Ok(Event::StartRecord {
            name: "JsonObject".into(),
            fields: length,
        }),
        JsonEvent::EndObject => Ok(Event::EndRecord),
        JsonEvent::Key(value) => Ok(Event::Field(value)),
    }
}

fn map_serialization_error(error: SerializationError) -> JsonError {
    let kind = match error {
        SerializationError::LimitExceeded => JsonErrorKind::LimitExceeded,
        SerializationError::TypeMismatch | SerializationError::UnexpectedEvent => {
            JsonErrorKind::TypeMismatch
        }
        SerializationError::EndOfInput | SerializationError::UnbalancedContainer => {
            JsonErrorKind::UnexpectedEof
        }
        SerializationError::DuplicateField => JsonErrorKind::DuplicateKey,
        SerializationError::InvalidContainerLength => JsonErrorKind::LimitExceeded,
    };
    JsonError::at_zero(kind)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use super::*;

    #[test]
    fn reader_and_dynamic_parser_cover_events_unicode_and_order() {
        let input = br#"{"z":[1,true,null],"a":{"text":"\uD83D\uDE80"}}"#;
        let value = parse(input).unwrap();
        assert_eq!(value.kind(), JsonKind::Object);
        assert_eq!(
            encode(&value).unwrap(),
            r#"{"z":[1,true,null],"a":{"text":"🚀"}}"#.as_bytes()
        );
        let mut reader = JsonReader::from_bytes(input, JsonDecodeOptions::default()).unwrap();
        let mut events = Vec::new();
        while let Some(event) = reader.next().unwrap() {
            events.push(reader.own(event).unwrap());
        }
        reader.finish().unwrap();
        assert!(matches!(events.first(), Some(JsonEvent::StartObject(None))));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, JsonEvent::String(text) if text == "🚀"))
        );
        assert!(reader.next().unwrap().is_none());
        let chunks = input.chunks(1).collect::<Vec<_>>();
        let chunked = parse_with_options(&chunks.concat(), JsonDecodeOptions::default()).unwrap();
        assert_eq!(chunked, value);
        let mut fragmented = JsonReader::from_chunks(chunks, JsonDecodeOptions::default()).unwrap();
        fragmented.finish().unwrap();
    }

    #[test]
    fn duplicate_policies_keep_first_position_and_reject_by_default() {
        let input = br#"{"a":1,"b":2,"a":3}"#;
        assert_eq!(parse(input).unwrap_err().kind, JsonErrorKind::DuplicateKey);
        let first = parse_with_options(
            input,
            JsonDecodeOptions {
                duplicate_keys: JsonDuplicatePolicy::First,
                ..Default::default()
            },
        )
        .unwrap();
        let last = parse_with_options(
            input,
            JsonDecodeOptions {
                duplicate_keys: JsonDuplicatePolicy::Last,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(encode(&first).unwrap(), br#"{"a":1,"b":2}"#);
        assert_eq!(encode(&last).unwrap(), br#"{"a":3,"b":2}"#);
    }

    #[test]
    fn numbers_are_lexical_exact_and_canonical_without_float_for_integers() {
        let number = JsonNumber::parse("1.2300").unwrap();
        assert_eq!(number.text(), "1.2300");
        assert_eq!(
            number.to_int().unwrap_err().kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(number.to_float64().unwrap(), 1.23);
        assert_eq!(
            JsonNumber::parse("-9223372036854775808")
                .unwrap()
                .to_int()
                .unwrap(),
            i64::MIN
        );
        assert_eq!(
            JsonNumber::parse("18446744073709551615")
                .unwrap()
                .to_uint()
                .unwrap(),
            u64::MAX
        );
        assert!(JsonNumber::parse("01").is_err());
        assert_eq!(
            encode_canonical(&JsonValue::Number(number)).unwrap(),
            b"1.23"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("-0").unwrap())).unwrap(),
            b"0"
        );
    }

    #[test]
    fn invalid_unicode_syntax_trailing_and_limits_are_terminal() {
        for input in [
            br#"[1,]"#.as_slice(),
            br#"{"a":1} x"#.as_slice(),
            br#"[1 2]"#.as_slice(),
        ] {
            assert!(matches!(
                parse(input).unwrap_err().kind,
                JsonErrorKind::InvalidSyntax | JsonErrorKind::TrailingData
            ));
        }
        for input in [
            br#""#.as_slice(),
            br#""\uD800""#.as_slice(),
            br#""\uDE00""#.as_slice(),
            br#""\q""#.as_slice(),
        ] {
            assert!(parse(input).is_err());
        }
        let mut limits = JsonLimits::default();
        limits.max_depth = 1;
        assert_eq!(
            parse_with_options(
                br#"[[0]]"#,
                JsonDecodeOptions {
                    limits,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        limits.max_array_items = 1;
        assert_eq!(
            parse_with_options(
                br#"[1,2]"#,
                JsonDecodeOptions {
                    limits,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        limits.max_string_bytes = 1;
        assert_eq!(
            parse_with_options(
                br#"["ab"]"#,
                JsonDecodeOptions {
                    limits,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        let mut reader = JsonReader::from_bytes(br#"["#, JsonDecodeOptions::default()).unwrap();
        assert!(reader.next().is_ok());
        assert_eq!(
            reader.finish().unwrap_err().kind,
            JsonErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn canonical_order_writer_and_output_limits_are_checked() {
        let value = JsonValue::Object(vec![
            JsonMember {
                key: "é".into(),
                value: JsonValue::Bool(true),
            },
            JsonMember {
                key: "a".into(),
                value: JsonValue::Null,
            },
        ]);
        assert_eq!(
            encode_canonical(&value).unwrap(),
            r#"{"a":null,"é":true}"#.as_bytes()
        );
        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::StartArray(None)).unwrap();
        writer
            .write(JsonEvent::Number(JsonNumber::parse("1").unwrap()))
            .unwrap();
        writer
            .write(JsonEvent::Number(JsonNumber::parse("2").unwrap()))
            .unwrap();
        writer.write(JsonEvent::EndArray).unwrap();
        assert_eq!(writer.finish().unwrap(), b"[1,2]");
        let mut canonical = JsonWriter::to_writer(JsonEncodeOptions {
            canonical: true,
            ..Default::default()
        })
        .unwrap();
        canonical.write(JsonEvent::StartObject(None)).unwrap();
        canonical.write(JsonEvent::Key("b".into())).unwrap();
        canonical.write(JsonEvent::Null).unwrap();
        assert_eq!(
            canonical
                .write(JsonEvent::Key("a".into()))
                .err()
                .unwrap()
                .kind,
            JsonErrorKind::CanonicalizationError
        );
        let mut tiny = JsonLimits::default();
        tiny.max_output_bytes = 2;
        assert_eq!(
            encode_with_options(
                &JsonValue::String("abc".into()),
                JsonEncodeOptions {
                    limits: tiny,
                    canonical: false
                }
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::LimitExceeded
        );
    }

    #[test]
    fn typed_paths_round_trip_scalars_options_and_arrays_without_dom() {
        assert_eq!(
            encode_typed(&42_i64, JsonEncodeOptions::default()).unwrap(),
            b"42"
        );
        assert_eq!(
            decode_typed::<i64>(b"42", JsonDecodeOptions::default()).unwrap(),
            42
        );
        assert_eq!(
            decode_typed::<Option<i64>>(b"null", JsonDecodeOptions::default()).unwrap(),
            None
        );
        assert_eq!(
            decode_typed::<Vec<i64>>(b"[1,2,3]", JsonDecodeOptions::default()).unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            decode_typed::<String>(br#""tondo""#, JsonDecodeOptions::default()).unwrap(),
            "tondo"
        );
        assert!(encode_typed(&vec![1_i64, 2], JsonEncodeOptions::default()).is_ok());
        assert!(decode_typed::<i64>(b"true", JsonDecodeOptions::default()).is_err());
    }

    #[test]
    fn reader_from_reader_and_error_locations_are_stable() {
        let mut reader = JsonReader::from_reader(
            std::io::Cursor::new(br#" true "#),
            JsonDecodeOptions::default(),
        )
        .unwrap();
        assert_eq!(reader.next().unwrap(), Some(JsonEvent::Bool(true)));
        reader.finish().unwrap();
        let error = parse(b"\n  ?").unwrap_err();
        assert_eq!(error.location.line, 2);
        assert_eq!(error.location.column, 3);
        assert_eq!(error.path.to_string(), "$");
        let nested = parse(b"[tru]").unwrap_err();
        assert_eq!(nested.path.to_string(), "$[0]");
    }

    #[test]
    fn public_models_and_number_boundaries_are_exhaustive() {
        assert_eq!(JsonValue::Null.kind(), JsonKind::Null);
        assert_eq!(JsonValue::Bool(true).kind(), JsonKind::Bool);
        assert_eq!(
            JsonValue::Number(JsonNumber::parse("1").unwrap()).kind(),
            JsonKind::Number
        );
        assert_eq!(JsonValue::String("x".into()).kind(), JsonKind::String);
        assert_eq!(JsonValue::Array(Vec::new()).kind(), JsonKind::Array);
        assert_eq!(JsonValue::Object(Vec::new()).kind(), JsonKind::Object);

        let mut path = JsonPath::root();
        assert!(path.segments().is_empty());
        path.0.push(JsonPathSegment::Key("a\"b".into()));
        path.0.push(JsonPathSegment::Index(2));
        assert_eq!(path.to_string(), "$[\"a\\\"b\"][2]");
        let error = JsonError::new(JsonErrorKind::InvalidSyntax, b"a\nb", 2, path.clone());
        assert_eq!(
            error.location,
            JsonLocation {
                offset: 2,
                line: 2,
                column: 1
            }
        );
        assert!(error.to_string().contains("InvalidSyntax"));

        for valid in ["0", "-0", "-12", "12", "1.25", "1e2", "1E+2", "1e-2"] {
            assert!(JsonNumber::parse(valid).is_ok(), "{valid}");
        }
        for invalid in [
            "", "-", "+1", "01", "1.", ".1", "1e", "1e+", "1e-", "1a", "--1",
        ] {
            assert_eq!(
                JsonNumber::parse(invalid).unwrap_err().kind,
                JsonErrorKind::InvalidNumber,
                "{invalid}"
            );
        }

        assert_eq!(JsonNumber::parse("0").unwrap().to_int().unwrap(), 0);
        assert_eq!(JsonNumber::parse("-0").unwrap().to_int().unwrap(), 0);
        assert_eq!(JsonNumber::parse("1.00").unwrap().to_int().unwrap(), 1);
        assert_eq!(JsonNumber::parse("1e2").unwrap().to_int().unwrap(), 100);
        assert_eq!(
            JsonNumber::parse("1e-2")
                .unwrap()
                .to_int()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1.2").unwrap().to_int().unwrap_err().kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("9223372036854775807")
                .unwrap()
                .to_int()
                .unwrap(),
            i64::MAX
        );
        assert_eq!(
            JsonNumber::parse("9223372036854775808")
                .unwrap()
                .to_int()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("-9223372036854775809")
                .unwrap()
                .to_int()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1e39")
                .unwrap()
                .to_int()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );

        assert_eq!(JsonNumber::parse("0").unwrap().to_uint().unwrap(), 0);
        assert_eq!(JsonNumber::parse("1.00").unwrap().to_uint().unwrap(), 1);
        assert_eq!(JsonNumber::parse("1e2").unwrap().to_uint().unwrap(), 100);
        assert_eq!(
            JsonNumber::parse("-1").unwrap().to_uint().unwrap_err().kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1.2")
                .unwrap()
                .to_uint()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("18446744073709551615")
                .unwrap()
                .to_uint()
                .unwrap(),
            u64::MAX
        );
        assert_eq!(
            JsonNumber::parse("18446744073709551616")
                .unwrap()
                .to_uint()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );

        assert_eq!(JsonNumber::parse("1.5").unwrap().to_float32().unwrap(), 1.5);
        assert_eq!(JsonNumber::parse("1.5").unwrap().to_float64().unwrap(), 1.5);
        assert_eq!(JsonNumber::parse("0").unwrap().to_float32().unwrap(), 0.0);
        assert_eq!(
            JsonNumber::parse("1e-10000")
                .unwrap()
                .to_float32()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1e-10000")
                .unwrap()
                .to_float64()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1e400")
                .unwrap()
                .to_float64()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            JsonNumber::parse("1e400")
                .unwrap()
                .to_float32()
                .unwrap_err()
                .kind,
            JsonErrorKind::NumberRange
        );
        assert_eq!(
            decimal_parts("1.20e+2").unwrap(),
            (false, b"120".to_vec(), 0)
        );
        assert_eq!(
            decimal_parts("-1.20e-2").unwrap(),
            (true, b"120".to_vec(), -4)
        );
        assert_eq!(
            decimal_parts("1x").unwrap_err().kind,
            JsonErrorKind::InvalidNumber
        );
        assert_eq!(
            decimal_parts("1e999999999999999999999").unwrap_err().kind,
            JsonErrorKind::NumberRange
        );
        assert!(parse_u128_digits(b"340282366920938463463374607431768211456").is_err());
    }

    #[test]
    fn reader_covers_utf8_syntax_limits_and_terminal_reuse() {
        let mut invalid_limits = JsonLimits::default();
        invalid_limits.max_document_bytes = 0;
        assert_eq!(
            JsonReader::from_bytes(
                b"0",
                JsonDecodeOptions {
                    limits: invalid_limits,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        let mut too_large = JsonLimits::default();
        too_large.max_document_bytes = 1;
        assert_eq!(
            JsonReader::from_chunks(
                [b"12".as_slice()],
                JsonDecodeOptions {
                    limits: too_large,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        assert_eq!(
            JsonReader::from_reader(
                std::io::Cursor::new(b"12"),
                JsonDecodeOptions {
                    limits: too_large,
                    ..Default::default()
                }
            )
            .err()
            .unwrap()
            .kind,
            JsonErrorKind::LimitExceeded
        );

        struct BrokenReader;
        impl Read for BrokenReader {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken"))
            }
        }
        assert_eq!(
            JsonReader::from_reader(BrokenReader, JsonDecodeOptions::default())
                .err()
                .unwrap()
                .kind,
            JsonErrorKind::IoError
        );

        let invalid_inputs: &[&[u8]] = &[
            b"{",
            b"[",
            b"{\"a\"",
            b"{\"a\":",
            b"{\"a\":1",
            b"[1",
            b"[1,",
            b"[1 2]",
            b"{\"a\" 1}",
            b"{\"a\":1,}",
            b"{\"a\":1 \"b\":2}",
            b"tru",
            b"fals",
            b"nul",
            b"01",
            b"-",
            b"1.",
            b"1e+",
            b"NaN",
            b"//",
            b"\"\x01\"",
        ];
        for input in invalid_inputs {
            assert!(parse(input).is_err(), "invalid input: {:?}", input);
        }
        assert_eq!(
            parse(&[b'"', 0xff, b'"']).unwrap_err().kind,
            JsonErrorKind::InvalidUtf8
        );
        assert_eq!(
            parse(&[b'"', 0xc2]).unwrap_err().kind,
            JsonErrorKind::UnexpectedEof
        );
        assert_eq!(
            parse(&[b'"', 0xc0, b'"']).unwrap_err().kind,
            JsonErrorKind::InvalidUtf8
        );
        assert_eq!(
            parse(b"1\xff").unwrap_err().kind,
            JsonErrorKind::InvalidUtf8
        );
        assert_eq!(
            parse(br#""\u12G4""#).unwrap_err().kind,
            JsonErrorKind::InvalidEscape
        );
        assert_eq!(
            parse(br#""\uD800x""#).unwrap_err().kind,
            JsonErrorKind::InvalidUnicodeScalar
        );

        let mut options = JsonDecodeOptions::default();
        options.limits.max_object_members = 1;
        assert_eq!(
            parse_with_options(br#"{"a":1,"b":2}"#, options)
                .unwrap_err()
                .kind,
            JsonErrorKind::LimitExceeded
        );
        options = JsonDecodeOptions::default();
        options.limits.max_events = 1;
        assert_eq!(
            parse_with_options(b"[0]", options).unwrap_err().kind,
            JsonErrorKind::LimitExceeded
        );
        options = JsonDecodeOptions::default();
        options.limits.max_number_bytes = 1;
        assert_eq!(
            parse_with_options(b"12", options).unwrap_err().kind,
            JsonErrorKind::LimitExceeded
        );
        options = JsonDecodeOptions::default();
        options.limits.max_document_bytes = 2;
        assert_eq!(
            parse_with_options(b" 0 ", options).unwrap_err().kind,
            JsonErrorKind::LimitExceeded
        );

        let mut reader = JsonReader::from_bytes(br#"[?]"#, JsonDecodeOptions::default()).unwrap();
        assert_eq!(reader.next().unwrap(), Some(JsonEvent::StartArray(None)));
        let error = reader.next().unwrap_err();
        assert_eq!(reader.next().unwrap_err(), error);
        assert_eq!(reader.finish().unwrap_err(), error);

        assert_eq!(validate(b" null "), Ok(()));
        assert_eq!(
            validate(b"").unwrap_err().kind,
            JsonErrorKind::UnexpectedEof
        );
        assert_eq!(
            validate(b"0 1").unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
    }

    #[test]
    fn reader_paths_and_all_unicode_escape_forms_are_stable() {
        let value = parse(br#"{"outer":[{"inner":"\\/\\b\\f\\n\\r\\t\\u0041\\u00e9"}]}"#).unwrap();
        assert!(matches!(value, JsonValue::Object(_)));
        let mut reader =
            JsonReader::from_bytes(br#"{"outer":[{"inner":?}]}"#, JsonDecodeOptions::default())
                .unwrap();
        while let Ok(Some(_)) = reader.next() {}
        let error = reader.next().unwrap_err();
        assert_eq!(error.path.to_string(), "$[\"outer\"][0][\"inner\"]");
        assert_eq!(error.location.line, 1);

        let mut reader = JsonReader::from_bytes(br#"[0]"#, JsonDecodeOptions::default()).unwrap();
        let event = reader.next().unwrap().unwrap();
        assert_eq!(reader.own(event.clone()).unwrap(), event);
        assert!(reader.next().unwrap().is_some());
        assert!(reader.next().unwrap().is_some());
        assert!(reader.next().unwrap().is_none());
        assert!(reader.next().unwrap().is_none());
    }

    #[test]
    fn dynamic_encoding_covers_values_order_numbers_strings_and_limits() {
        let values = [
            JsonValue::Null,
            JsonValue::Bool(false),
            JsonValue::Number(JsonNumber::parse("12").unwrap()),
            JsonValue::String("\"\\\u{08}\u{0c}\n\r\t\u{01}é".into()),
            JsonValue::Array(vec![JsonValue::Null, JsonValue::Bool(true)]),
            JsonValue::Object(vec![JsonMember {
                key: "x".into(),
                value: JsonValue::Null,
            }]),
        ];
        for value in &values {
            let encoded = encode(value).unwrap();
            assert_eq!(parse(&encoded).unwrap(), *value);
        }
        assert_eq!(
            encode(&JsonValue::String("\"\\\u{08}\u{0c}\n\r\t\u{01}".into())).unwrap(),
            br#""\"\\\b\f\n\r\t\u0001""#
        );
        assert_eq!(
            canonicalize(br#" {"b":1.2300,"a":-0} "#).unwrap(),
            br#"{"a":0,"b":1.23}"#
        );
        assert_eq!(
            encode_canonical_with_limits(&JsonValue::Null, JsonLimits::default()).unwrap(),
            b"null"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("0.0010").unwrap())).unwrap(),
            b"0.001"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("12300").unwrap())).unwrap(),
            b"12300"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("1e3").unwrap())).unwrap(),
            b"1000"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("1e-3").unwrap())).unwrap(),
            b"0.001"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Number(JsonNumber::parse("-1.20e2").unwrap())).unwrap(),
            b"-120"
        );
        assert_eq!(
            encode_canonical(&JsonValue::Object(vec![
                JsonMember {
                    key: "a".into(),
                    value: JsonValue::Null
                },
                JsonMember {
                    key: "a".into(),
                    value: JsonValue::Bool(true)
                },
            ]))
            .unwrap_err()
            .kind,
            JsonErrorKind::CanonicalizationError
        );
        let mut invalid = JsonLimits::default();
        invalid.max_output_bytes = 0;
        assert_eq!(
            encode_with_options(
                &JsonValue::Null,
                JsonEncodeOptions {
                    limits: invalid,
                    canonical: false
                }
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        let mut tiny = JsonLimits::default();
        tiny.max_output_bytes = 1;
        assert_eq!(
            encode_with_options(
                &JsonValue::String("x".into()),
                JsonEncodeOptions {
                    limits: tiny,
                    canonical: false
                }
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        assert_eq!(member_order(&[], false).unwrap(), Vec::<usize>::new());
        assert_eq!(
            member_order(
                &[JsonMember {
                    key: "b".into(),
                    value: JsonValue::Null
                }],
                false
            )
            .unwrap(),
            vec![0]
        );
    }

    #[test]
    fn streaming_writer_rejects_invalid_sequences_and_is_terminal() {
        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        assert_eq!(
            writer.finish().unwrap_err().kind,
            JsonErrorKind::UnexpectedEof
        );
        assert_eq!(
            writer.write(JsonEvent::Null).unwrap_err().kind,
            JsonErrorKind::UnexpectedEof
        );

        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        for event in [
            JsonEvent::EndArray,
            JsonEvent::EndObject,
            JsonEvent::Key("x".into()),
        ] {
            assert_eq!(
                writer.write(event).unwrap_err().kind,
                JsonErrorKind::InvalidSyntax
            );
        }
        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::StartObject(None)).unwrap();
        assert_eq!(
            writer
                .write(JsonEvent::String("value".into()))
                .unwrap_err()
                .kind,
            JsonErrorKind::InvalidSyntax
        );
        writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::StartObject(None)).unwrap();
        writer.write(JsonEvent::Key("x".into())).unwrap();
        assert_eq!(
            writer.write(JsonEvent::Key("y".into())).unwrap_err().kind,
            JsonErrorKind::InvalidSyntax
        );

        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::StartObject(None)).unwrap();
        writer.write(JsonEvent::Key("x".into())).unwrap();
        writer.write(JsonEvent::Null).unwrap();
        assert_eq!(
            writer.write(JsonEvent::Key("x".into())).unwrap_err().kind,
            JsonErrorKind::DuplicateKey
        );
        assert_eq!(
            writer.write(JsonEvent::EndObject).unwrap_err().kind,
            JsonErrorKind::DuplicateKey
        );

        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::Null).unwrap();
        assert_eq!(
            writer.write(JsonEvent::Bool(true)).unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
        assert_eq!(
            writer.finish().unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
        assert_eq!(
            writer.write(JsonEvent::Null).unwrap_err().kind,
            JsonErrorKind::TrailingData
        );
        let mut finished = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        finished.write(JsonEvent::Null).unwrap();
        assert_eq!(finished.finish().unwrap(), b"null");
        assert_eq!(
            finished.finish().unwrap_err().kind,
            JsonErrorKind::InvalidSyntax
        );

        let mut writer = JsonWriter::to_writer(JsonEncodeOptions::default()).unwrap();
        writer.write(JsonEvent::StartObject(None)).unwrap();
        writer.write(JsonEvent::Key("a".into())).unwrap();
        writer.write(JsonEvent::Null).unwrap();
        writer.write(JsonEvent::Key("b".into())).unwrap();
        writer.write(JsonEvent::Bool(true)).unwrap();
        writer.write(JsonEvent::EndObject).unwrap();
        assert_eq!(writer.finish().unwrap(), br#"{"a":null,"b":true}"#);

        let mut limited = JsonLimits::default();
        limited.max_output_bytes = 1;
        let mut writer = JsonWriter::to_writer(JsonEncodeOptions {
            limits: limited,
            canonical: false,
        })
        .unwrap();
        assert_eq!(
            writer
                .write(JsonEvent::String("x".into()))
                .unwrap_err()
                .kind,
            JsonErrorKind::LimitExceeded
        );
        assert_eq!(
            writer.write(JsonEvent::Null).unwrap_err().kind,
            JsonErrorKind::LimitExceeded
        );
        assert_eq!(
            writer.finish().unwrap_err().kind,
            JsonErrorKind::LimitExceeded
        );
    }

    #[test]
    fn typed_event_adapters_and_serialization_errors_are_exhaustive() {
        for event in [
            Event::Null,
            Event::Bool(true),
            Event::Int(-1),
            Event::UInt(1),
            Event::Float(1.5),
            Event::Float32(1.5f32.to_bits()),
            Event::Float64(1.5f64.to_bits()),
            Event::String("x".into()),
            Event::StartArray(Some(1)),
            Event::EndArray,
            Event::StartRecord {
                name: "R".into(),
                fields: Some(1),
            },
            Event::Field("x".into()),
            Event::EndRecord,
            Event::StartMap(Some(1)),
            Event::EndMap,
        ] {
            assert!(event_to_json(event).is_ok());
        }
        for event in [
            Event::MapKey,
            Event::Bytes(vec![1]),
            Event::StartEnum {
                name: "E".into(),
                variant: "V".into(),
            },
            Event::EndEnum,
        ] {
            assert_eq!(
                event_to_json(event).unwrap_err().kind,
                JsonErrorKind::TypeMismatch
            );
        }
        for event in [
            JsonEvent::Null,
            JsonEvent::Bool(true),
            JsonEvent::Number(JsonNumber::parse("-1").unwrap()),
            JsonEvent::Number(JsonNumber::parse("1e2").unwrap()),
            JsonEvent::String("x".into()),
            JsonEvent::StartArray(Some(1)),
            JsonEvent::EndArray,
            JsonEvent::StartObject(Some(1)),
            JsonEvent::EndObject,
            JsonEvent::Key("x".into()),
        ] {
            assert!(json_to_event(event).is_ok());
        }
        for error in [
            SerializationError::LimitExceeded,
            SerializationError::TypeMismatch,
            SerializationError::UnexpectedEvent,
            SerializationError::EndOfInput,
            SerializationError::UnbalancedContainer,
            SerializationError::DuplicateField,
            SerializationError::InvalidContainerLength,
        ] {
            let mapped = map_serialization_error(error);
            assert!(matches!(
                mapped.kind,
                JsonErrorKind::LimitExceeded
                    | JsonErrorKind::TypeMismatch
                    | JsonErrorKind::UnexpectedEof
                    | JsonErrorKind::DuplicateKey
            ));
        }
        assert_eq!(
            decode_typed::<u64>(b"1", JsonDecodeOptions::default()).unwrap(),
            1
        );
        assert_eq!(
            decode_typed::<f32>(b"1.5", JsonDecodeOptions::default()).unwrap(),
            1.5
        );
        assert_eq!(
            decode_typed::<f64>(b"1.5", JsonDecodeOptions::default()).unwrap(),
            1.5
        );
        assert_eq!(
            decode_typed::<f64>(b"1", JsonDecodeOptions::default()).unwrap(),
            1.0
        );
        let float32_events = [Event::Float32(1.25f32.to_bits())];
        let mut float32_source = serialization::EventDeserializer::new(
            &float32_events,
            serialization::Limits::default(),
        )
        .unwrap();
        assert_eq!(f64::deserialize(&mut float32_source).unwrap(), 1.25);
        let float64_events = [Event::Float64(1.25f64.to_bits())];
        let mut float64_source = serialization::EventDeserializer::new(
            &float64_events,
            serialization::Limits::default(),
        )
        .unwrap();
        assert_eq!(f32::deserialize(&mut float64_source).unwrap(), 1.25);
        let non_finite_events = [Event::Float64(f64::INFINITY.to_bits())];
        let mut non_finite = serialization::EventDeserializer::new(
            &non_finite_events,
            serialization::Limits::default(),
        )
        .unwrap();
        assert_eq!(
            f32::deserialize(&mut non_finite).unwrap_err(),
            SerializationError::TypeMismatch
        );
        assert_eq!(
            decode_typed::<u64>(b"-1", JsonDecodeOptions::default())
                .unwrap_err()
                .kind,
            JsonErrorKind::TypeMismatch
        );
        assert_eq!(
            decode_typed::<f32>(b"1", JsonDecodeOptions::default()).unwrap(),
            1.0
        );
        assert_eq!(
            decode_typed::<String>(b"{}", JsonDecodeOptions::default())
                .unwrap_err()
                .kind,
            JsonErrorKind::TypeMismatch
        );
        assert_eq!(
            encode_typed(&true, JsonEncodeOptions::default()).unwrap(),
            b"true"
        );
        assert_eq!(
            encode_typed(&1.5f32, JsonEncodeOptions::default()).unwrap(),
            b"1.5"
        );
        assert_eq!(
            encode_typed(&1.5f64, JsonEncodeOptions::default()).unwrap(),
            b"1.5"
        );
        assert_eq!(
            encode_typed(&Some(1_i64), JsonEncodeOptions::default()).unwrap(),
            b"1"
        );
    }

    #[test]
    fn dynamic_collector_attach_and_reader_from_chunks_cover_error_paths() {
        let mut frames = vec![BuildFrame::Object {
            members: Vec::new(),
            pending_key: None,
        }];
        let mut root = None;
        assert_eq!(
            attach_value(
                JsonValue::Null,
                &mut frames,
                &mut root,
                JsonDuplicatePolicy::Reject
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::InvalidSyntax
        );
        if let BuildFrame::Object { pending_key, .. } = frames.last_mut().unwrap() {
            *pending_key = Some("x".into());
        }
        attach_value(
            JsonValue::Null,
            &mut frames,
            &mut root,
            JsonDuplicatePolicy::Reject,
        )
        .unwrap();
        if let BuildFrame::Object { pending_key, .. } = frames.last_mut().unwrap() {
            *pending_key = Some("x".into());
        }
        assert_eq!(
            attach_value(
                JsonValue::Bool(true),
                &mut frames,
                &mut root,
                JsonDuplicatePolicy::Reject
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::DuplicateKey
        );
        if let BuildFrame::Object { pending_key, .. } = frames.last_mut().unwrap() {
            *pending_key = Some("x".into());
        }
        attach_value(
            JsonValue::Bool(true),
            &mut frames,
            &mut root,
            JsonDuplicatePolicy::First,
        )
        .unwrap();
        if let BuildFrame::Object { pending_key, .. } = frames.last_mut().unwrap() {
            *pending_key = Some("x".into());
        }
        attach_value(
            JsonValue::Bool(false),
            &mut frames,
            &mut root,
            JsonDuplicatePolicy::Last,
        )
        .unwrap();
        assert_eq!(frames.len(), 1);

        let mut array = vec![BuildFrame::Array(Vec::new())];
        attach_value(
            JsonValue::Null,
            &mut array,
            &mut root,
            JsonDuplicatePolicy::Reject,
        )
        .unwrap();
        attach_value(
            JsonValue::Null,
            &mut [],
            &mut root,
            JsonDuplicatePolicy::Reject,
        )
        .unwrap();
        assert!(root.is_some());
        assert_eq!(
            attach_value(
                JsonValue::Null,
                &mut [],
                &mut root,
                JsonDuplicatePolicy::Reject
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::TrailingData
        );

        let mut invalid = JsonLimits::default();
        invalid.max_document_bytes = 0;
        assert_eq!(
            canonicalize_with_options(
                b"0",
                JsonDecodeOptions {
                    limits: invalid,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .kind,
            JsonErrorKind::LimitExceeded
        );
        assert_eq!(
            parse_with_options(b"", JsonDecodeOptions::default())
                .unwrap_err()
                .kind,
            JsonErrorKind::UnexpectedEof
        );
        let chunks = [b"[".as_slice(), b"0]".as_slice()];
        let mut reader = JsonReader::from_chunks(chunks, JsonDecodeOptions::default()).unwrap();
        reader.finish().unwrap();
    }
}
