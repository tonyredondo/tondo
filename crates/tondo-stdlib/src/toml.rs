//! Deterministic TOML 1.1.0 value, parser and event kernel.
//!
//! This module owns the data-format boundary only.  It deliberately does not
//! read tondo.toml, consult a clock/time-zone database, or expose a compiler
//! intrinsic.  The parser builds a value atomically and all limits are checked
//! before the value or encoded bytes are returned.

use std::collections::HashSet;
use std::fmt;
use std::io::Read;

use crate::serialization::{
    self, Decode, Decoder, Encode, Encoder, Event, SerializationError, Toml as TomlCodec,
};

/// Codec identity used by the common typed serialization protocol.
pub type Toml = TomlCodec;

/// A civil TOML date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TomlDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

/// A civil TOML time with nanosecond precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TomlTime {
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub nanosecond: u32,
}

/// A local civil date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TomlDateTime {
    pub date: TomlDate,
    pub time: TomlTime,
}

/// A local date/time paired with a fixed offset in minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TomlOffsetDateTime {
    pub local: TomlDateTime,
    pub offset_minutes: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TomlMember {
    pub key: String,
    pub value: TomlValue,
}

/// Lossless dynamic TOML model.
#[derive(Debug, Clone, PartialEq)]
pub enum TomlValue {
    /// TOML has no null wire value; this variant is retained only for the
    /// common serialization adapter and is rejected by the TOML encoder.
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    OffsetDateTime(TomlOffsetDateTime),
    LocalDateTime(TomlDateTime),
    LocalDate(TomlDate),
    LocalTime(TomlTime),
    Array(Vec<TomlValue>),
    Table(Vec<TomlMember>),
}

/// Borrowed source view.  The value is materialised only when requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TomlValueView<'a> {
    input: &'a [u8],
    options: TomlOptions,
}

impl<'a> TomlValueView<'a> {
    pub fn bytes(self) -> &'a [u8] {
        self.input
    }

    pub fn clone_value(self) -> Result<TomlValue, TomlError> {
        parse(self.input, self.options)
    }
}

pub type ValueView<'a> = TomlValueView<'a>;

#[derive(Debug, Clone, PartialEq)]
pub enum TomlScalar {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    OffsetDateTime(TomlOffsetDateTime),
    LocalDateTime(TomlDateTime),
    LocalDate(TomlDate),
    LocalTime(TomlTime),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TomlEvent {
    StreamStart,
    TableStart(Vec<String>),
    ArrayTableStart(Vec<String>),
    TableEnd,
    Key(Vec<String>),
    Scalar(TomlScalar),
    ArrayStart,
    ArrayEnd,
    InlineTableStart,
    InlineTableEnd,
    StreamEnd,
}

/// Finite parser and materialisation limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TomlLimits {
    pub max_input_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_tables: usize,
    pub max_array_elements: usize,
    pub max_key_bytes: usize,
    pub max_path_segments: usize,
    pub max_scalar_bytes: usize,
    pub max_string_bytes: usize,
    pub max_array_table_rows: usize,
}

impl Default for TomlLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_depth: 256,
            max_nodes: 1_048_576,
            max_tables: 1_048_576,
            max_array_elements: 1_048_576,
            max_key_bytes: 4096,
            max_path_segments: 256,
            max_scalar_bytes: 64 * 1024 * 1024,
            max_string_bytes: 64 * 1024 * 1024,
            max_array_table_rows: 65_536,
        }
    }
}

impl TomlLimits {
    pub fn defaults() -> Self {
        Self::default()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create(
        max_input_bytes: usize,
        max_depth: usize,
        max_nodes: usize,
        max_tables: usize,
        max_array_elements: usize,
        max_key_bytes: usize,
        max_path_segments: usize,
        max_scalar_bytes: usize,
        max_string_bytes: usize,
        max_array_table_rows: usize,
    ) -> Result<Self, TomlError> {
        let limits = Self {
            max_input_bytes,
            max_depth,
            max_nodes,
            max_tables,
            max_array_elements,
            max_key_bytes,
            max_path_segments,
            max_scalar_bytes,
            max_string_bytes,
            max_array_table_rows,
        };
        if limits.valid() {
            Ok(limits)
        } else {
            Err(TomlError::at_zero(TomlErrorKind::InvalidLimit))
        }
    }

    fn valid(self) -> bool {
        self.max_input_bytes > 0
            && self.max_depth > 0
            && self.max_nodes > 0
            && self.max_tables > 0
            && self.max_array_elements > 0
            && self.max_key_bytes > 0
            && self.max_path_segments > 0
            && self.max_scalar_bytes > 0
            && self.max_string_bytes > 0
            && self.max_array_table_rows > 0
            && self.max_nodes.checked_add(self.max_tables).is_some()
            && self
                .max_nodes
                .checked_add(self.max_array_elements)
                .is_some()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TomlOptions {
    pub limits: TomlLimits,
}

impl TomlOptions {
    pub fn defaults() -> Self {
        Self::default()
    }

    pub fn create(limits: TomlLimits) -> Self {
        Self { limits }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TomlPathSegment {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TomlSpan {
    pub start_offset: usize,
    pub end_offset: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TomlErrorKind {
    InvalidLimit,
    InvalidUtf8,
    InvalidCharacter,
    InvalidComment,
    InvalidKey,
    EmptyKey,
    InvalidEscape,
    InvalidString,
    InvalidNumber,
    IntegerOutOfRange,
    InvalidDateTime,
    DateTimePrecision,
    InvalidArray,
    InvalidTable,
    InvalidTableArray,
    DuplicateKey,
    DuplicateTable,
    TableAfterValue,
    InlineTableExtension,
    MissingValue,
    UnexpectedToken,
    TrailingInput,
    DepthLimit,
    NodeLimit,
    TableLimit,
    ArrayLimit,
    KeyLimit,
    ScalarLimit,
    StringLimit,
    ResourceLimit,
    TypeMismatch,
    MissingField,
    UnknownField,
    Io(String),
    Closed,
    NoProgress,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TomlError {
    pub kind: TomlErrorKind,
    pub span: TomlSpan,
    pub path: Vec<TomlPathSegment>,
}

impl TomlError {
    fn at_zero(kind: TomlErrorKind) -> Self {
        Self {
            kind,
            span: TomlSpan {
                start_line: 1,
                start_column: 1,
                end_line: 1,
                end_column: 1,
                ..TomlSpan::default()
            },
            path: Vec::new(),
        }
    }

    fn at(kind: TomlErrorKind, input: &[u8], start: usize, end: usize) -> Self {
        fn location(input: &[u8], offset: usize) -> (usize, usize) {
            let mut line = 1;
            let mut column = 1;
            for byte in &input[..offset.min(input.len())] {
                if *byte == b'\n' {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            (line, column)
        }
        let start = start.min(input.len());
        let end = end.max(start).min(input.len());
        let (start_line, start_column) = location(input, start);
        let (end_line, end_column) = location(input, end);
        Self {
            kind,
            span: TomlSpan {
                start_offset: start,
                end_offset: end,
                start_line,
                start_column,
                end_line,
                end_column,
            },
            path: Vec::new(),
        }
    }

    fn with_path(mut self, path: &[String]) -> Self {
        self.path = path.iter().cloned().map(TomlPathSegment::Key).collect();
        self
    }
}

impl fmt::Display for TomlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "TOML {:?} at {}:{}",
            self.kind, self.span.start_line, self.span.start_column
        )
    }
}

impl std::error::Error for TomlError {}

impl From<SerializationError> for TomlError {
    fn from(error: SerializationError) -> Self {
        let kind = match error {
            SerializationError::EndOfInput => TomlErrorKind::UnexpectedToken,
            SerializationError::LimitExceeded => TomlErrorKind::NodeLimit,
            SerializationError::DuplicateField => TomlErrorKind::DuplicateKey,
            SerializationError::MissingField => TomlErrorKind::MissingField,
            SerializationError::UnknownField => TomlErrorKind::UnknownField,
            SerializationError::TypeMismatch => TomlErrorKind::TypeMismatch,
            SerializationError::UnexpectedEvent
            | SerializationError::UnbalancedContainer
            | SerializationError::InvalidContainerLength => TomlErrorKind::UnexpectedToken,
        };
        Self::at_zero(kind)
    }
}

struct Parser<'a> {
    input: &'a [u8],
    options: TomlOptions,
    pos: usize,
    root: TomlValue,
    current: Vec<String>,
    explicit_tables: HashSet<Vec<String>>,
    explicit_array_tables: HashSet<Vec<String>>,
    inline_paths: Vec<Vec<String>>,
    nodes: usize,
    tables: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a [u8], options: TomlOptions) -> Result<Self, TomlError> {
        if !options.limits.valid() {
            return Err(TomlError::at_zero(TomlErrorKind::InvalidLimit));
        }
        if input.len() > options.limits.max_input_bytes {
            return Err(TomlError::at_zero(TomlErrorKind::ResourceLimit));
        }
        std::str::from_utf8(input)
            .map_err(|_| TomlError::at(TomlErrorKind::InvalidUtf8, input, 0, input.len()))?;
        Ok(Self {
            input,
            options,
            pos: 0,
            root: TomlValue::Table(Vec::new()),
            current: Vec::new(),
            explicit_tables: HashSet::new(),
            explicit_array_tables: HashSet::new(),
            inline_paths: Vec::new(),
            nodes: 1,
            tables: 1,
        })
    }

    fn parse(mut self) -> Result<TomlValue, TomlError> {
        self.skip_trivia();
        while self.pos < self.input.len() {
            let start = self.pos;
            if self.input[self.pos..].starts_with(b"[[") {
                self.parse_header(true, start)?;
            } else if self.input[self.pos] == b'[' {
                self.parse_header(false, start)?;
            } else {
                self.parse_assignment(start)?;
            }
            self.skip_trivia();
            if self.pos == start {
                return Err(self.error(TomlErrorKind::UnexpectedToken, start));
            }
        }
        validate_value(&self.root, self.options.limits, 0)?;
        Ok(self.root)
    }

    fn error(&self, kind: TomlErrorKind, start: usize) -> TomlError {
        TomlError::at(kind, self.input, start, self.pos.max(start))
    }

    fn skip_inline(&mut self) {
        while matches!(self.input.get(self.pos), Some(b' ' | b'\t')) {
            self.pos += 1;
        }
    }

    fn skip_trivia(&mut self) {
        loop {
            while matches!(self.input.get(self.pos), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                self.pos += 1;
            }
            if self.input.get(self.pos) != Some(&b'#') {
                break;
            }
            while let Some(byte) = self.input.get(self.pos) {
                self.pos += 1;
                if *byte == b'\n' {
                    break;
                }
            }
        }
    }

    fn parse_header(&mut self, array: bool, start: usize) -> Result<(), TomlError> {
        self.pos += if array { 2 } else { 1 };
        self.skip_inline();
        let path = self.parse_key_path()?;
        if path.is_empty() || path.len() > self.options.limits.max_path_segments {
            return Err(self.error(TomlErrorKind::KeyLimit, start));
        }
        self.skip_inline();
        let closing: &[u8] = if array { b"]]" } else { b"]" };
        if !self.input[self.pos..].starts_with(closing) {
            return Err(self.error(
                if array {
                    TomlErrorKind::InvalidTableArray
                } else {
                    TomlErrorKind::InvalidTable
                },
                start,
            ));
        }
        self.pos += closing.len();
        self.skip_inline();
        if self.input.get(self.pos) == Some(&b'#') {
            self.skip_trivia();
        } else if !matches!(self.input.get(self.pos), None | Some(b'\r' | b'\n')) {
            return Err(self.error(TomlErrorKind::TrailingInput, start));
        }
        if array {
            self.open_array_table(&path, start)?;
        } else {
            self.open_table(&path, start)?;
        }
        self.current = path;
        Ok(())
    }

    fn parse_assignment(&mut self, start: usize) -> Result<(), TomlError> {
        let key_path = self.parse_key_path()?;
        if key_path.is_empty() {
            return Err(self.error(TomlErrorKind::EmptyKey, start));
        }
        self.skip_inline();
        if self.input.get(self.pos) != Some(&b'=') {
            return Err(self.error(TomlErrorKind::UnexpectedToken, start));
        }
        self.pos += 1;
        self.skip_inline();
        if matches!(self.input.get(self.pos), None | Some(b'\r' | b'\n' | b'#')) {
            return Err(self.error(TomlErrorKind::MissingValue, start));
        }
        let value = self.parse_value(0)?;
        self.skip_inline();
        if self.input.get(self.pos) == Some(&b'#') {
            self.skip_trivia();
        } else if !matches!(self.input.get(self.pos), None | Some(b'\r' | b'\n')) {
            return Err(self.error(TomlErrorKind::TrailingInput, start));
        }
        let mut path = self.current.clone();
        path.extend(key_path);
        if path.len() > self.options.limits.max_path_segments {
            return Err(self.error(TomlErrorKind::KeyLimit, start));
        }
        if self
            .inline_paths
            .iter()
            .any(|inline| path.starts_with(inline))
            && !path_has_array_table(&self.root, &path)
        {
            return Err(self
                .error(TomlErrorKind::InlineTableExtension, start)
                .with_path(&path));
        }
        let is_inline = matches!(value, TomlValue::Table(_));
        assign_value(&mut self.root, &path, value)
            .map_err(|kind| self.error(kind, start).with_path(&path))?;
        if is_inline {
            self.inline_paths.push(path);
        }
        Ok(())
    }

    fn parse_key_path(&mut self) -> Result<Vec<String>, TomlError> {
        let mut path = Vec::new();
        loop {
            self.skip_inline();
            let key = self.parse_key()?;
            if key.len() > self.options.limits.max_key_bytes {
                return Err(self.error(TomlErrorKind::KeyLimit, self.pos));
            }
            path.push(key);
            if path.len() > self.options.limits.max_path_segments {
                return Err(self.error(TomlErrorKind::KeyLimit, self.pos));
            }
            self.skip_inline();
            if self.input.get(self.pos) != Some(&b'.') {
                break;
            }
            self.pos += 1;
        }
        Ok(path)
    }

    fn parse_key(&mut self) -> Result<String, TomlError> {
        let start = self.pos;
        match self.input.get(self.pos) {
            Some(b'"') => {
                if self.input[self.pos..].starts_with(b"\"\"\"") {
                    return Err(self.error(TomlErrorKind::InvalidKey, start));
                }
                self.parse_string(b'"', false)
            }
            Some(b'\'') => {
                if self.input[self.pos..].starts_with(b"'''") {
                    return Err(self.error(TomlErrorKind::InvalidKey, start));
                }
                self.parse_string(b'\'', false)
            }
            Some(byte) if is_bare_key(*byte) => {
                self.pos += 1;
                while let Some(byte) = self.input.get(self.pos) {
                    if !is_bare_key(*byte) {
                        break;
                    }
                    self.pos += 1;
                }
                String::from_utf8(self.input[start..self.pos].to_vec())
                    .map_err(|_| self.error(TomlErrorKind::InvalidKey, start))
            }
            Some(_) => Err(self.error(TomlErrorKind::InvalidKey, start)),
            None => Err(self.error(TomlErrorKind::EmptyKey, start)),
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<TomlValue, TomlError> {
        if depth >= self.options.limits.max_depth {
            return Err(self.error(TomlErrorKind::DepthLimit, self.pos));
        }
        self.note_node()?;
        match self.input.get(self.pos) {
            Some(b'"') => {
                let multiline = self.input[self.pos..].starts_with(b"\"\"\"");
                Ok(TomlValue::Text(self.parse_string(b'"', multiline)?))
            }
            Some(b'\'') => {
                let multiline = self.input[self.pos..].starts_with(b"'''");
                Ok(TomlValue::Text(self.parse_string(b'\'', multiline)?))
            }
            Some(b'[') => self.parse_array(depth),
            Some(b'{') => self.parse_inline_table(depth),
            Some(_) => {
                let start = self.pos;
                let token = self.parse_token();
                self.parse_scalar_token(&token, start)
            }
            None => Err(self.error(TomlErrorKind::MissingValue, self.pos)),
        }
    }

    fn note_node(&mut self) -> Result<(), TomlError> {
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > self.options.limits.max_nodes {
            return Err(self.error(TomlErrorKind::NodeLimit, self.pos));
        }
        Ok(())
    }

    fn parse_array(&mut self, depth: usize) -> Result<TomlValue, TomlError> {
        self.pos += 1;
        let mut values = Vec::new();
        self.skip_trivia();
        if self.input.get(self.pos) == Some(&b']') {
            self.pos += 1;
            return Ok(TomlValue::Array(values));
        }
        loop {
            if values.len() >= self.options.limits.max_array_elements {
                return Err(self.error(TomlErrorKind::ArrayLimit, self.pos));
            }
            values.push(self.parse_value(depth + 1)?);
            self.skip_trivia();
            match self.input.get(self.pos) {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_trivia();
                    if self.input.get(self.pos) == Some(&b']') {
                        self.pos += 1;
                        break;
                    }
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.error(TomlErrorKind::InvalidArray, self.pos)),
            }
        }
        Ok(TomlValue::Array(values))
    }

    fn parse_inline_table(&mut self, depth: usize) -> Result<TomlValue, TomlError> {
        self.pos += 1;
        let mut members = Vec::new();
        self.skip_trivia();
        if self.input.get(self.pos) == Some(&b'}') {
            self.pos += 1;
            return Ok(TomlValue::Table(members));
        }
        loop {
            let path = self.parse_key_path()?;
            self.skip_inline();
            if self.input.get(self.pos) != Some(&b'=') {
                return Err(self.error(TomlErrorKind::UnexpectedToken, self.pos));
            }
            self.pos += 1;
            self.skip_inline();
            if matches!(
                self.input.get(self.pos),
                None | Some(b',' | b'}' | b'\r' | b'\n')
            ) {
                return Err(self.error(TomlErrorKind::MissingValue, self.pos));
            }
            let value = self.parse_value(depth + 1)?;
            insert_table_value(&mut members, &path, value)
                .map_err(|kind| self.error(kind, self.pos))?;
            self.skip_trivia();
            match self.input.get(self.pos) {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_trivia();
                    if self.input.get(self.pos) == Some(&b'}') {
                        self.pos += 1;
                        break;
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.error(TomlErrorKind::InvalidArray, self.pos)),
            }
        }
        Ok(TomlValue::Table(members))
    }

    fn parse_token(&mut self) -> String {
        let start = self.pos;
        while let Some(byte) = self.input.get(self.pos) {
            if matches!(*byte, b',' | b']' | b'}' | b'#' | b'\r' | b'\n') {
                break;
            }
            self.pos += 1;
        }
        String::from_utf8_lossy(&self.input[start..self.pos])
            .trim()
            .to_owned()
    }

    fn parse_scalar_token(&self, token: &str, start: usize) -> Result<TomlValue, TomlError> {
        match token {
            "true" => return Ok(TomlValue::Bool(true)),
            "false" => return Ok(TomlValue::Bool(false)),
            _ => {}
        }
        if let Some(result) = parse_temporal(token) {
            return result.map_err(|kind| TomlError::at(kind, self.input, start, self.pos));
        }
        if token.is_empty() {
            return Err(self.error(TomlErrorKind::InvalidNumber, start));
        }
        parse_number(token).map_err(|kind| TomlError::at(kind, self.input, start, self.pos))
    }

    fn parse_string(&mut self, quote: u8, multiline: bool) -> Result<String, TomlError> {
        let start = self.pos;
        let delimiter_len = if multiline { 3 } else { 1 };
        self.pos += delimiter_len;
        if multiline && self.input.get(self.pos) == Some(&b'\r') {
            self.pos += 1;
            if self.input.get(self.pos) == Some(&b'\n') {
                self.pos += 1;
            }
        } else if multiline && self.input.get(self.pos) == Some(&b'\n') {
            self.pos += 1;
        }
        let mut output = Vec::new();
        loop {
            if self.pos >= self.input.len() {
                return Err(self.error(TomlErrorKind::InvalidString, start));
            }
            if multiline {
                let closing = [quote, quote, quote];
                if self.input[self.pos..].starts_with(&closing) {
                    self.pos += 3;
                    break;
                }
            } else if self.input[self.pos] == quote {
                self.pos += 1;
                break;
            }
            let byte = self.input[self.pos];
            if quote == b'"' && byte == b'\\' {
                self.pos += 1;
                let Some(next) = self.input.get(self.pos).copied() else {
                    return Err(self.error(TomlErrorKind::InvalidEscape, start));
                };
                if multiline && matches!(next, b'\n' | b'\r') {
                    if next == b'\r' {
                        self.pos += 1;
                        if self.input.get(self.pos) == Some(&b'\n') {
                            self.pos += 1;
                        }
                    } else {
                        self.pos += 1;
                    }
                    while matches!(self.input.get(self.pos), Some(b' ' | b'\t' | b'\r' | b'\n')) {
                        self.pos += 1;
                    }
                    continue;
                }
                output.extend(self.decode_escape(next, start)?);
                self.pos += 1;
                continue;
            }
            if !multiline && (byte == b'\n' || byte == b'\r') {
                return Err(self.error(TomlErrorKind::InvalidString, start));
            }
            if byte == b'\r' && multiline {
                self.pos += 1;
                if self.input.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
                output.push(b'\n');
                continue;
            }
            if byte < 0x20 && byte != b'\t' && !(multiline && byte == b'\n') {
                return Err(self.error(TomlErrorKind::InvalidString, start));
            }
            if quote == b'\'' && !multiline && byte == b'\'' {
                return Err(self.error(TomlErrorKind::InvalidString, start));
            }
            output.push(byte);
            self.pos += 1;
        }
        String::from_utf8(output).map_err(|_| self.error(TomlErrorKind::InvalidString, start))
    }

    fn decode_escape(&mut self, next: u8, start: usize) -> Result<Vec<u8>, TomlError> {
        let value = match next {
            b'b' => vec![0x08],
            b't' => vec![b'\t'],
            b'n' => vec![b'\n'],
            b'f' => vec![0x0c],
            b'r' => vec![b'\r'],
            b'e' => vec![0x1b],
            b'"' => vec![b'"'],
            b'\\' => vec![b'\\'],
            b'x' => vec![self.parse_escape_scalar(2, start)? as u8],
            b'u' => {
                let scalar = self.parse_escape_scalar(4, start)?;
                let character = char::from_u32(scalar)
                    .ok_or_else(|| self.error(TomlErrorKind::InvalidEscape, start))?;
                let mut bytes = [0; 4];
                character.encode_utf8(&mut bytes).as_bytes().to_vec()
            }
            b'U' => {
                let scalar = self.parse_escape_scalar(8, start)?;
                let character = char::from_u32(scalar)
                    .ok_or_else(|| self.error(TomlErrorKind::InvalidEscape, start))?;
                let mut bytes = [0; 4];
                character.encode_utf8(&mut bytes).as_bytes().to_vec()
            }
            _ => return Err(self.error(TomlErrorKind::InvalidEscape, start)),
        };
        Ok(value)
    }

    fn parse_escape_scalar(&mut self, digits: usize, start: usize) -> Result<u32, TomlError> {
        if self.pos + digits >= self.input.len() {
            return Err(self.error(TomlErrorKind::InvalidEscape, start));
        }
        let mut value = 0_u32;
        for byte in &self.input[self.pos + 1..self.pos + 1 + digits] {
            let digit = (*byte as char)
                .to_digit(16)
                .ok_or_else(|| self.error(TomlErrorKind::InvalidEscape, start))?;
            value = value
                .checked_mul(16)
                .and_then(|current| current.checked_add(digit))
                .ok_or_else(|| self.error(TomlErrorKind::InvalidEscape, start))?;
        }
        self.pos += digits;
        Ok(value)
    }

    fn open_table(&mut self, path: &[String], start: usize) -> Result<(), TomlError> {
        if self.explicit_tables.contains(path) {
            return Err(self
                .error(TomlErrorKind::DuplicateTable, start)
                .with_path(path));
        }
        if self.explicit_array_tables.contains(path) {
            return Err(self
                .error(TomlErrorKind::InvalidTable, start)
                .with_path(path));
        }
        if self
            .inline_paths
            .iter()
            .any(|inline| path.starts_with(inline))
            && !path_has_array_table(&self.root, path)
        {
            return Err(self
                .error(TomlErrorKind::InlineTableExtension, start)
                .with_path(path));
        }
        let result = table_mut_at(&mut self.root, path, true);
        if let Err(kind) = result {
            return Err(self.error(kind, start).with_path(path));
        }
        self.explicit_tables.insert(path.to_vec());
        Ok(())
    }

    fn open_array_table(&mut self, path: &[String], start: usize) -> Result<(), TomlError> {
        if self.explicit_tables.contains(path) {
            return Err(self
                .error(TomlErrorKind::InvalidTableArray, start)
                .with_path(path));
        }
        if self
            .inline_paths
            .iter()
            .any(|inline| path.starts_with(inline))
        {
            return Err(self
                .error(TomlErrorKind::InlineTableExtension, start)
                .with_path(path));
        }
        let parent_path = &path[..path.len() - 1];
        let key = &path[path.len() - 1];
        let parent_result = table_mut_at(&mut self.root, parent_path, true);
        let parent = match parent_result {
            Ok(parent) => parent,
            Err(kind) => return Err(self.error(kind, start).with_path(path)),
        };
        let Some(index) = member_index(parent, key) else {
            if self.tables >= self.options.limits.max_tables {
                return Err(self.error(TomlErrorKind::TableLimit, start));
            }
            self.tables += 1;
            parent.push(TomlMember {
                key: key.clone(),
                value: TomlValue::Array(vec![TomlValue::Table(Vec::new())]),
            });
            self.explicit_array_tables.insert(path.to_vec());
            return Ok(());
        };
        match &mut parent[index].value {
            TomlValue::Array(rows)
                if rows
                    .iter()
                    .all(|value| matches!(value, TomlValue::Table(_))) =>
            {
                if rows.len() >= self.options.limits.max_array_table_rows {
                    return Err(self.error(TomlErrorKind::TableLimit, start));
                }
                if self.tables >= self.options.limits.max_tables {
                    return Err(self.error(TomlErrorKind::TableLimit, start));
                }
                self.tables += 1;
                rows.push(TomlValue::Table(Vec::new()));
                self.explicit_array_tables.insert(path.to_vec());
                Ok(())
            }
            TomlValue::Table(_) => Err(self.error(TomlErrorKind::InvalidTableArray, start)),
            _ => Err(self.error(TomlErrorKind::InvalidTableArray, start)),
        }
    }
}

fn is_bare_key(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
}

fn member_index(members: &[TomlMember], key: &str) -> Option<usize> {
    members.iter().position(|member| member.key == key)
}

fn path_has_array_table(value: &TomlValue, path: &[String]) -> bool {
    let mut current = value;
    for key in path {
        let TomlValue::Table(members) = current else {
            return false;
        };
        let Some(member) = members.iter().find(|member| member.key == *key) else {
            return false;
        };
        match &member.value {
            TomlValue::Array(rows)
                if !rows.is_empty()
                    && rows.iter().all(|row| matches!(row, TomlValue::Table(_))) =>
            {
                return true;
            }
            next => current = next,
        }
    }
    false
}

fn table_mut_at<'a>(
    value: &'a mut TomlValue,
    path: &[String],
    create: bool,
) -> Result<&'a mut Vec<TomlMember>, TomlErrorKind> {
    if path.is_empty() {
        return match value {
            TomlValue::Table(members) => Ok(members),
            _ => Err(TomlErrorKind::TableAfterValue),
        };
    }
    let TomlValue::Table(members) = value else {
        return Err(TomlErrorKind::TableAfterValue);
    };
    let key = &path[0];
    let index = if let Some(index) = member_index(members, key) {
        index
    } else if create {
        members.push(TomlMember {
            key: key.clone(),
            value: TomlValue::Table(Vec::new()),
        });
        members.len() - 1
    } else {
        return Err(TomlErrorKind::TableAfterValue);
    };
    let child = &mut members[index].value;
    match child {
        TomlValue::Table(_) => table_mut_at(child, &path[1..], create),
        TomlValue::Array(rows) => {
            let Some(last) = rows.last_mut() else {
                return Err(TomlErrorKind::TableAfterValue);
            };
            if !matches!(last, TomlValue::Table(_)) {
                return Err(TomlErrorKind::TableAfterValue);
            }
            table_mut_at(last, &path[1..], create)
        }
        _ => Err(TomlErrorKind::TableAfterValue),
    }
}

fn insert_table_value(
    members: &mut Vec<TomlMember>,
    path: &[String],
    value: TomlValue,
) -> Result<(), TomlErrorKind> {
    if path.is_empty() {
        return Err(TomlErrorKind::EmptyKey);
    }
    if path.len() == 1 {
        if member_index(members, &path[0]).is_some() {
            return Err(TomlErrorKind::DuplicateKey);
        }
        members.push(TomlMember {
            key: path[0].clone(),
            value,
        });
        return Ok(());
    }
    let index = if let Some(index) = member_index(members, &path[0]) {
        index
    } else {
        members.push(TomlMember {
            key: path[0].clone(),
            value: TomlValue::Table(Vec::new()),
        });
        members.len() - 1
    };
    match &mut members[index].value {
        TomlValue::Table(child) => insert_table_value(child, &path[1..], value),
        _ => Err(TomlErrorKind::TableAfterValue),
    }
}

fn assign_value(
    root: &mut TomlValue,
    path: &[String],
    value: TomlValue,
) -> Result<(), TomlErrorKind> {
    if path.is_empty() {
        return Err(TomlErrorKind::EmptyKey);
    }
    let parent = table_mut_at(root, &path[..path.len() - 1], true)?;
    let key = &path[path.len() - 1];
    if let Some(index) = member_index(parent, key) {
        if matches!(
            parent[index].value,
            TomlValue::Table(_) | TomlValue::Array(_)
        ) {
            return Err(TomlErrorKind::TableAfterValue);
        }
        return Err(TomlErrorKind::DuplicateKey);
    }
    parent.push(TomlMember {
        key: key.clone(),
        value,
    });
    Ok(())
}

fn validate_value(value: &TomlValue, limits: TomlLimits, depth: usize) -> Result<(), TomlError> {
    let mut nodes = 0;
    let mut tables = 0;
    validate_value_inner(value, limits, depth, &mut nodes, &mut tables)
}

fn validate_value_inner(
    value: &TomlValue,
    limits: TomlLimits,
    depth: usize,
    nodes: &mut usize,
    tables: &mut usize,
) -> Result<(), TomlError> {
    if depth >= limits.max_depth {
        return Err(TomlError::at_zero(TomlErrorKind::DepthLimit));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > limits.max_nodes {
        return Err(TomlError::at_zero(TomlErrorKind::NodeLimit));
    }
    match value {
        TomlValue::Null => {}
        TomlValue::Bool(_) | TomlValue::Int(_) | TomlValue::UInt(_) | TomlValue::Float(_) => {}
        TomlValue::Text(text) => {
            if text.len() > limits.max_string_bytes {
                return Err(TomlError::at_zero(TomlErrorKind::StringLimit));
            }
            if text.len() > limits.max_scalar_bytes {
                return Err(TomlError::at_zero(TomlErrorKind::ScalarLimit));
            }
        }
        TomlValue::OffsetDateTime(_)
        | TomlValue::LocalDateTime(_)
        | TomlValue::LocalDate(_)
        | TomlValue::LocalTime(_) => {}
        TomlValue::Array(values) => {
            if values.len() > limits.max_array_elements {
                return Err(TomlError::at_zero(TomlErrorKind::ArrayLimit));
            }
            if !values.is_empty()
                && values
                    .iter()
                    .all(|item| matches!(item, TomlValue::Table(_)))
                && values.len() > limits.max_array_table_rows
            {
                return Err(TomlError::at_zero(TomlErrorKind::TableLimit));
            }
            for item in values {
                validate_value_inner(item, limits, depth + 1, nodes, tables)?;
            }
        }
        TomlValue::Table(members) => {
            *tables = tables.saturating_add(1);
            if *tables > limits.max_tables {
                return Err(TomlError::at_zero(TomlErrorKind::TableLimit));
            }
            if members.len() > limits.max_array_elements {
                return Err(TomlError::at_zero(TomlErrorKind::TableLimit));
            }
            let mut keys = HashSet::new();
            for member in members {
                if member.key.len() > limits.max_key_bytes {
                    return Err(TomlError::at_zero(TomlErrorKind::KeyLimit));
                }
                if !keys.insert(&member.key) {
                    return Err(TomlError::at_zero(TomlErrorKind::DuplicateKey));
                }
                validate_value_inner(&member.value, limits, depth + 1, nodes, tables)?;
            }
        }
    }
    Ok(())
}

/// Parse one UTF-8 TOML 1.1.0 document.
pub fn parse(input: &[u8], options: TomlOptions) -> Result<TomlValue, TomlError> {
    Parser::new(input, options)?.parse()
}

/// Parse a borrowed view without retaining a materialised tree.
pub fn parse_view<'a>(
    input: &'a [u8],
    options: TomlOptions,
) -> Result<TomlValueView<'a>, TomlError> {
    let _ = parse(input, options)?;
    Ok(TomlValueView { input, options })
}

pub fn validate(input: &[u8], options: TomlOptions) -> Result<(), TomlError> {
    let _ = parse(input, options)?;
    Ok(())
}

fn parse_date(token: &str) -> Result<TomlDate, TomlErrorKind> {
    let bytes = token.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return Err(TomlErrorKind::InvalidDateTime);
    }
    let year = parse_digits(&bytes[..4]).ok_or(TomlErrorKind::InvalidDateTime)? as i32;
    let month = parse_digits(&bytes[5..7]).ok_or(TomlErrorKind::InvalidDateTime)? as u8;
    let day = parse_digits(&bytes[8..]).ok_or(TomlErrorKind::InvalidDateTime)? as u8;
    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
        return Err(TomlErrorKind::InvalidDateTime);
    }
    Ok(TomlDate { year, month, day })
}

fn parse_time(token: &str) -> Result<TomlTime, TomlErrorKind> {
    let (base, fraction) = token.split_once('.').unwrap_or((token, ""));
    let parts: Vec<&str> = base.split(':').collect();
    if !(parts.len() == 2 || parts.len() == 3) {
        return Err(TomlErrorKind::InvalidDateTime);
    }
    let hour = parse_digits(parts[0].as_bytes()).ok_or(TomlErrorKind::InvalidDateTime)? as u8;
    let minute = parse_digits(parts[1].as_bytes()).ok_or(TomlErrorKind::InvalidDateTime)? as u8;
    let second = if parts.len() == 3 {
        parse_digits(parts[2].as_bytes()).ok_or(TomlErrorKind::InvalidDateTime)? as u8
    } else {
        0
    };
    if hour > 23 || minute > 59 || second > 59 {
        return Err(TomlErrorKind::InvalidDateTime);
    }
    if fraction.is_empty() {
        return Ok(TomlTime {
            hour,
            minute,
            second,
            nanosecond: 0,
        });
    }
    if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(if fraction.len() > 9 {
            TomlErrorKind::DateTimePrecision
        } else {
            TomlErrorKind::InvalidDateTime
        });
    }
    let mut nanos = fraction
        .parse::<u32>()
        .map_err(|_| TomlErrorKind::InvalidDateTime)?;
    for _ in fraction.len()..9 {
        nanos *= 10;
    }
    Ok(TomlTime {
        hour,
        minute,
        second,
        nanosecond: nanos,
    })
}

fn parse_temporal(token: &str) -> Option<Result<TomlValue, TomlErrorKind>> {
    let bytes = token.as_bytes();
    if bytes.len() >= 10 && bytes.get(4) == Some(&b'-') && bytes.get(7) == Some(&b'-') {
        let date = match parse_date(&token[..10]) {
            Ok(date) => date,
            Err(kind) => return Some(Err(kind)),
        };
        if token.len() == 10 {
            return Some(Ok(TomlValue::LocalDate(date)));
        }
        let rest = &token[10..];
        if !matches!(rest.as_bytes().first(), Some(b'T' | b' ')) {
            return Some(Err(TomlErrorKind::InvalidDateTime));
        }
        let mut datetime = rest[1..].to_owned();
        let mut offset_minutes = None;
        if datetime.ends_with('Z') {
            datetime.pop();
            offset_minutes = Some(0);
        } else if let Some(index) = datetime.rfind(['+', '-'])
            && index > 0
        {
            let offset = &datetime[index..];
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let pieces: Vec<&str> = offset[1..].split(':').collect();
            if pieces.len() != 2 {
                return Some(Err(TomlErrorKind::InvalidDateTime));
            }
            let hour = match parse_digits(pieces[0].as_bytes()) {
                Some(value) => value,
                None => return Some(Err(TomlErrorKind::InvalidDateTime)),
            };
            let minute = match parse_digits(pieces[1].as_bytes()) {
                Some(value) => value,
                None => return Some(Err(TomlErrorKind::InvalidDateTime)),
            };
            if hour > 23 || minute > 59 {
                return Some(Err(TomlErrorKind::InvalidDateTime));
            }
            offset_minutes = Some(sign * (hour as i32 * 60 + minute as i32));
            datetime.truncate(index);
        }
        let time = match parse_time(&datetime) {
            Ok(time) => time,
            Err(kind) => return Some(Err(kind)),
        };
        let value = TomlDateTime { date, time };
        return Some(Ok(match offset_minutes {
            Some(offset_minutes) => TomlValue::OffsetDateTime(TomlOffsetDateTime {
                local: value,
                offset_minutes,
            }),
            None => TomlValue::LocalDateTime(value),
        }));
    }
    if bytes.len() >= 5 && bytes.get(2) == Some(&b':') {
        return Some(parse_time(token).map(TomlValue::LocalTime));
    }
    None
}

fn parse_number(token: &str) -> Result<TomlValue, TomlErrorKind> {
    let bytes = token.as_bytes();
    if bytes.is_empty() {
        return Err(TomlErrorKind::InvalidNumber);
    }
    let signed = matches!(bytes[0], b'+' | b'-');
    let sign = if bytes[0] == b'-' { -1_i8 } else { 1_i8 };
    let body = if signed { &token[1..] } else { token };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return Err(TomlErrorKind::InvalidNumber);
    }
    for (index, byte) in body.bytes().enumerate() {
        if byte == b'_' {
            let previous = body.as_bytes().get(index.wrapping_sub(1)).copied();
            let next = body.as_bytes().get(index + 1).copied();
            if !previous.is_some_and(|byte| byte.is_ascii_alphanumeric())
                || !next.is_some_and(|byte| byte.is_ascii_alphanumeric())
            {
                return Err(TomlErrorKind::InvalidNumber);
            }
        }
    }
    let normalized = body.replace('_', "");
    if !normalized.starts_with("0.")
        && !normalized.starts_with("0e")
        && !normalized.starts_with("0E")
        && normalized.starts_with('0')
        && normalized.len() > 1
        && !normalized.starts_with("0x")
        && !normalized.starts_with("0o")
        && !normalized.starts_with("0b")
    {
        return Err(TomlErrorKind::InvalidNumber);
    }
    if normalized.eq_ignore_ascii_case("inf") || normalized.eq_ignore_ascii_case("nan") {
        let value = if normalized.eq_ignore_ascii_case("inf") {
            if sign < 0 {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }
        } else {
            f64::NAN
        };
        return Ok(TomlValue::Float(value));
    }
    if normalized.starts_with("0x") || normalized.starts_with("0o") || normalized.starts_with("0b")
    {
        let radix = match &normalized[..2] {
            "0x" => 16,
            "0o" => 8,
            _ => 2,
        };
        let digits = &normalized[2..];
        if digits.is_empty() || !digits.chars().all(|character| character.is_digit(radix)) {
            return Err(TomlErrorKind::InvalidNumber);
        }
        let magnitude =
            u64::from_str_radix(digits, radix).map_err(|_| TomlErrorKind::IntegerOutOfRange)?;
        if sign < 0 {
            if magnitude > (i64::MAX as u64) + 1 {
                return Err(TomlErrorKind::IntegerOutOfRange);
            }
            if magnitude == (i64::MAX as u64) + 1 {
                Ok(TomlValue::Int(i64::MIN))
            } else {
                Ok(TomlValue::Int(-(magnitude as i64)))
            }
        } else if magnitude <= i64::MAX as u64 {
            Ok(TomlValue::Int(magnitude as i64))
        } else {
            Ok(TomlValue::UInt(magnitude))
        }
    } else if normalized.contains('.') || normalized.contains('e') || normalized.contains('E') {
        if !valid_decimal_float(&normalized) {
            return Err(TomlErrorKind::InvalidNumber);
        }
        let value = token
            .parse::<f64>()
            .map_err(|_| TomlErrorKind::InvalidNumber)?;
        Ok(TomlValue::Float(value))
    } else {
        if !normalized
            .chars()
            .all(|character| character.is_ascii_digit())
        {
            return Err(TomlErrorKind::InvalidNumber);
        }
        let magnitude = normalized
            .parse::<u64>()
            .map_err(|_| TomlErrorKind::IntegerOutOfRange)?;
        if sign < 0 {
            if magnitude > (i64::MAX as u64) + 1 {
                return Err(TomlErrorKind::IntegerOutOfRange);
            }
            if magnitude == (i64::MAX as u64) + 1 {
                Ok(TomlValue::Int(i64::MIN))
            } else {
                Ok(TomlValue::Int(-(magnitude as i64)))
            }
        } else if magnitude <= i64::MAX as u64 {
            Ok(TomlValue::Int(magnitude as i64))
        } else {
            Ok(TomlValue::UInt(magnitude))
        }
    }
}

fn valid_decimal_float(value: &str) -> bool {
    let (mantissa, exponent) = if let Some(index) = value.find(['e', 'E']) {
        if value[index + 1..].contains(['e', 'E']) {
            return false;
        }
        (&value[..index], Some(&value[index + 1..]))
    } else {
        (value, None)
    };
    if let Some(exponent) = exponent {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        if exponent.is_empty() || !exponent.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    let (whole, fraction) = if let Some(index) = mantissa.find('.') {
        if mantissa[index + 1..].contains('.') {
            return false;
        }
        (&mantissa[..index], Some(&mantissa[index + 1..]))
    } else {
        (mantissa, None)
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if let Some(fraction) = fraction
        && (fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    true
}

fn parse_digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value = 0_u32;
    for byte in bytes {
        value = value
            .checked_mul(10)?
            .checked_add(u32::from(*byte - b'0'))?;
    }
    Some(value)
}

fn is_leap_year(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

fn bare_key(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(is_bare_key)
}

fn write_key(key: &str, output: &mut String) {
    if bare_key(key) {
        output.push_str(key);
    } else {
        output.push('"');
        escape_string(key, output);
        output.push('"');
    }
}

fn escape_string(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '\u{08}' => output.push_str("\\b"),
            '\t' => output.push_str("\\t"),
            '\n' => output.push_str("\\n"),
            '\u{0c}' => output.push_str("\\f"),
            '\r' => output.push_str("\\r"),
            '\u{1b}' => output.push_str("\\e"),
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(output, "\\u{:04X}", character as u32);
            }
            character => output.push(character),
        }
    }
}

fn scalar_to_text(value: &TomlValue) -> Result<String, TomlErrorKind> {
    match value {
        TomlValue::Bool(value) => Ok(value.to_string()),
        TomlValue::Int(value) => Ok(value.to_string()),
        TomlValue::UInt(value) => Ok(value.to_string()),
        TomlValue::Float(value) => {
            if value.is_nan() {
                Ok("nan".to_owned())
            } else if value.is_infinite() {
                Ok(if value.is_sign_negative() {
                    "-inf".to_owned()
                } else {
                    "inf".to_owned()
                })
            } else {
                let mut text = value.to_string();
                if !text.contains('.') && !text.contains('e') && !text.contains('E') {
                    text.push_str(".0");
                }
                Ok(text)
            }
        }
        TomlValue::Text(value) => {
            let mut output = String::with_capacity(value.len() + 2);
            output.push('"');
            escape_string(value, &mut output);
            output.push('"');
            Ok(output)
        }
        TomlValue::OffsetDateTime(value) => Ok(format_offset_datetime(value)),
        TomlValue::LocalDateTime(value) => Ok(format_datetime(value)),
        TomlValue::LocalDate(value) => Ok(format_date(value)),
        TomlValue::LocalTime(value) => Ok(format_time(value)),
        TomlValue::Null | TomlValue::Array(_) | TomlValue::Table(_) => {
            Err(TomlErrorKind::TypeMismatch)
        }
    }
}

fn format_date(value: &TomlDate) -> String {
    format!("{:04}-{:02}-{:02}", value.year, value.month, value.day)
}

fn format_time(value: &TomlTime) -> String {
    if value.nanosecond == 0 {
        format!("{:02}:{:02}:{:02}", value.hour, value.minute, value.second)
    } else {
        let mut fraction = format!("{:09}", value.nanosecond);
        while fraction.ends_with('0') {
            fraction.pop();
        }
        format!(
            "{:02}:{:02}:{:02}.{}",
            value.hour, value.minute, value.second, fraction
        )
    }
}

fn format_datetime(value: &TomlDateTime) -> String {
    format!("{}T{}", format_date(&value.date), format_time(&value.time))
}

fn format_offset_datetime(value: &TomlOffsetDateTime) -> String {
    let local = format_datetime(&value.local);
    if value.offset_minutes == 0 {
        return format!("{local}Z");
    }
    let sign = if value.offset_minutes < 0 { '-' } else { '+' };
    let minutes = value.offset_minutes.unsigned_abs();
    format!("{local}{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

fn value_to_inline(value: &TomlValue, canonical: bool) -> Result<String, TomlErrorKind> {
    match value {
        TomlValue::Array(values) => {
            let mut output = String::from("[");
            for (index, item) in values.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                output.push_str(&value_to_inline(item, canonical)?);
            }
            output.push(']');
            Ok(output)
        }
        TomlValue::Table(members) => {
            let mut output = String::from("{");
            let mut ordered: Vec<&TomlMember> = members.iter().collect();
            if canonical {
                ordered.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
            }
            for (index, member) in ordered.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                write_key(&member.key, &mut output);
                output.push_str(" = ");
                output.push_str(&value_to_inline(&member.value, canonical)?);
            }
            output.push('}');
            Ok(output)
        }
        _ => scalar_to_text(value),
    }
}

fn array_of_tables(value: &TomlValue) -> Option<&[TomlValue]> {
    match value {
        TomlValue::Array(values)
            if !values.is_empty()
                && values
                    .iter()
                    .all(|item| matches!(item, TomlValue::Table(_))) =>
        {
            Some(values)
        }
        _ => None,
    }
}

fn path_text(path: &[String]) -> String {
    path.iter()
        .map(|key| {
            if bare_key(key) {
                key.clone()
            } else {
                let mut output = String::from("\"");
                escape_string(key, &mut output);
                output.push('"');
                output
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

fn write_table_body(
    members: &[TomlMember],
    path: &[String],
    output: &mut String,
    canonical: bool,
) -> Result<(), TomlErrorKind> {
    let mut ordered: Vec<&TomlMember> = members.iter().collect();
    if canonical {
        ordered.sort_by(|left, right| left.key.as_bytes().cmp(right.key.as_bytes()));
    }
    for member in &ordered {
        if matches!(&member.value, TomlValue::Table(_)) || array_of_tables(&member.value).is_some()
        {
            continue;
        }
        write_key(&member.key, output);
        output.push_str(" = ");
        output.push_str(&value_to_inline(&member.value, canonical)?);
        output.push('\n');
    }
    for member in ordered {
        if let TomlValue::Table(child) = &member.value {
            if !output.is_empty() && !output.ends_with("\n\n") {
                output.push('\n');
            }
            let mut child_path = path.to_vec();
            child_path.push(member.key.clone());
            output.push('[');
            output.push_str(&path_text(&child_path));
            output.push_str("]\n");
            write_table_body(child, &child_path, output, canonical)?;
        } else if let Some(rows) = array_of_tables(&member.value) {
            let mut row_path = path.to_vec();
            row_path.push(member.key.clone());
            for row in rows {
                if !output.is_empty() && !output.ends_with("\n\n") {
                    output.push('\n');
                }
                output.push_str("[[");
                output.push_str(&path_text(&row_path));
                output.push_str("]]\n");
                let TomlValue::Table(row_members) = row else {
                    return Err(TomlErrorKind::InvalidTableArray);
                };
                write_table_body(row_members, &row_path, output, canonical)?;
            }
        }
    }
    Ok(())
}

fn encode_inner(value: &TomlValue, canonical: bool) -> Result<Vec<u8>, TomlError> {
    let TomlValue::Table(members) = value else {
        return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch));
    };
    let mut output = String::new();
    write_table_body(members, &[], &mut output, canonical).map_err(TomlError::at_zero)?;
    Ok(output.into_bytes())
}

/// Encode the dynamic TOML value using insertion order.
pub fn encode(value: &TomlValue, options: TomlOptions) -> Result<Vec<u8>, TomlError> {
    if !options.limits.valid() {
        return Err(TomlError::at_zero(TomlErrorKind::InvalidLimit));
    }
    validate_value(value, options.limits, 0)?;
    encode_inner(value, false)
}

/// Encode the dynamic value with reproducible key ordering.
pub fn encode_canonical(value: &TomlValue, limits: TomlLimits) -> Result<Vec<u8>, TomlError> {
    if !limits.valid() {
        return Err(TomlError::at_zero(TomlErrorKind::InvalidLimit));
    }
    validate_value(value, limits, 0)?;
    encode_inner(value, true)
}

fn scalar_to_event(value: &TomlValue) -> Result<TomlEvent, TomlError> {
    let scalar = match value {
        TomlValue::Bool(value) => TomlScalar::Bool(*value),
        TomlValue::Int(value) => TomlScalar::Int(*value),
        TomlValue::UInt(value) => TomlScalar::UInt(*value),
        TomlValue::Float(value) => TomlScalar::Float(*value),
        TomlValue::Text(value) => TomlScalar::Text(value.clone()),
        TomlValue::OffsetDateTime(value) => TomlScalar::OffsetDateTime(*value),
        TomlValue::LocalDateTime(value) => TomlScalar::LocalDateTime(*value),
        TomlValue::LocalDate(value) => TomlScalar::LocalDate(*value),
        TomlValue::LocalTime(value) => TomlScalar::LocalTime(*value),
        TomlValue::Null | TomlValue::Array(_) | TomlValue::Table(_) => {
            return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch));
        }
    };
    Ok(TomlEvent::Scalar(scalar))
}

fn emit_value(value: &TomlValue, events: &mut Vec<TomlEvent>) -> Result<(), TomlError> {
    match value {
        TomlValue::Array(values) => {
            events.push(TomlEvent::ArrayStart);
            for item in values {
                if matches!(item, TomlValue::Table(_)) {
                    events.push(TomlEvent::InlineTableStart);
                    let TomlValue::Table(members) = item else {
                        unreachable!()
                    };
                    for member in members {
                        events.push(TomlEvent::Key(vec![member.key.clone()]));
                        emit_value(&member.value, events)?;
                    }
                    events.push(TomlEvent::InlineTableEnd);
                } else {
                    emit_value(item, events)?;
                }
            }
            events.push(TomlEvent::ArrayEnd);
        }
        TomlValue::Table(members) => {
            events.push(TomlEvent::InlineTableStart);
            for member in members {
                events.push(TomlEvent::Key(vec![member.key.clone()]));
                emit_value(&member.value, events)?;
            }
            events.push(TomlEvent::InlineTableEnd);
        }
        _ => events.push(scalar_to_event(value)?),
    }
    Ok(())
}

fn emit_table(
    members: &[TomlMember],
    path: &[String],
    events: &mut Vec<TomlEvent>,
) -> Result<(), TomlError> {
    for member in members {
        let mut member_path = path.to_vec();
        member_path.push(member.key.clone());
        match &member.value {
            TomlValue::Table(child) => {
                events.push(TomlEvent::TableStart(member_path.clone()));
                emit_table(child, &member_path, events)?;
                events.push(TomlEvent::TableEnd);
            }
            value if array_of_tables(value).is_some() => {
                let rows = array_of_tables(value).unwrap_or_default();
                for row in rows {
                    events.push(TomlEvent::ArrayTableStart(member_path.clone()));
                    let TomlValue::Table(row_members) = row else {
                        return Err(TomlError::at_zero(TomlErrorKind::InvalidTableArray));
                    };
                    emit_table(row_members, &member_path, events)?;
                    events.push(TomlEvent::TableEnd);
                }
            }
            value => {
                events.push(TomlEvent::Key(member_path));
                emit_value(value, events)?;
            }
        }
    }
    Ok(())
}

fn events_for_value(value: &TomlValue) -> Result<Vec<TomlEvent>, TomlError> {
    let TomlValue::Table(members) = value else {
        return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch));
    };
    let mut events = vec![TomlEvent::StreamStart];
    emit_table(members, &[], &mut events)?;
    events.push(TomlEvent::StreamEnd);
    Ok(events)
}

fn common_events_for_value(value: &TomlValue, events: &mut Vec<Event>) -> Result<(), TomlError> {
    match value {
        TomlValue::Table(members) => {
            events.push(Event::StartMap(Some(members.len())));
            for member in members {
                events.push(Event::MapKey);
                events.push(Event::String(member.key.clone()));
                common_events_for_value(&member.value, events)?;
            }
            events.push(Event::EndMap);
        }
        TomlValue::Array(values) => {
            events.push(Event::StartArray(Some(values.len())));
            for item in values {
                common_events_for_value(item, events)?;
            }
            events.push(Event::EndArray);
        }
        TomlValue::Bool(value) => events.push(Event::Bool(*value)),
        TomlValue::Int(value) => events.push(Event::Int(i128::from(*value))),
        TomlValue::UInt(value) => events.push(Event::UInt(u128::from(*value))),
        TomlValue::Float(value) => events.push(Event::Float64(value.to_bits())),
        TomlValue::Text(value) => events.push(Event::String(value.clone())),
        TomlValue::OffsetDateTime(value) => {
            events.push(Event::String(format_offset_datetime(value)))
        }
        TomlValue::LocalDateTime(value) => events.push(Event::String(format_datetime(value))),
        TomlValue::LocalDate(value) => events.push(Event::String(format_date(value))),
        TomlValue::LocalTime(value) => events.push(Event::String(format_time(value))),
        TomlValue::Null => return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch)),
    }
    Ok(())
}

/// Materialise a value through the common typed event protocol.
pub fn encode_typed<T: serialization::Serialize>(
    value: &T,
    options: TomlOptions,
) -> Result<Vec<u8>, TomlError> {
    let events = serialization::serialize_value(
        value,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_nodes,
            max_bytes: options.limits.max_input_bytes,
            max_container_items: options.limits.max_array_elements,
        },
    )
    .map_err(TomlError::from)?;
    let value = common_events_to_value(&events)?;
    encode(&value, options)
}

/// Decode a value through the common typed event protocol.
pub fn decode_typed<T: serialization::Deserialize>(
    input: &[u8],
    options: TomlOptions,
) -> Result<T, TomlError> {
    let value = parse(input, options)?;
    let mut events = Vec::new();
    common_events_for_value(&value, &mut events)?;
    serialization::deserialize_value(
        &events,
        serialization::Limits {
            max_depth: options.limits.max_depth,
            max_events: options.limits.max_nodes,
            max_bytes: options.limits.max_input_bytes,
            max_container_items: options.limits.max_array_elements,
        },
    )
    .map_err(TomlError::from)
}

/// Bounded materialising event reader.  The source is parsed once so public
/// event delivery is invariant under input chunking.
#[derive(Debug)]
pub struct TomlReader {
    events: Vec<TomlEvent>,
    index: usize,
    common_events: Vec<Event>,
    common_index: usize,
    options: TomlOptions,
    eof_returned: bool,
    finished: bool,
    terminal: Option<TomlError>,
}

impl TomlReader {
    pub fn from_bytes(input: &[u8], options: TomlOptions) -> Result<Self, TomlError> {
        let value = parse(input, options)?;
        Self::from_value(value, options)
    }

    pub fn from_reader<R: Read>(mut input: R, options: TomlOptions) -> Result<Self, TomlError> {
        if !options.limits.valid() {
            return Err(TomlError::at_zero(TomlErrorKind::InvalidLimit));
        }
        let mut bytes = Vec::new();
        let mut chunk = [0_u8; 8192];
        loop {
            let read = input
                .read(&mut chunk)
                .map_err(|error| TomlError::at_zero(TomlErrorKind::Io(error.to_string())))?;
            if read == 0 {
                break;
            }
            if bytes
                .len()
                .checked_add(read)
                .is_none_or(|length| length > options.limits.max_input_bytes)
            {
                return Err(TomlError::at_zero(TomlErrorKind::ResourceLimit));
            }
            bytes.extend_from_slice(&chunk[..read]);
        }
        Self::from_bytes(&bytes, options)
    }

    pub fn from_chunks<I, B>(chunks: I, options: TomlOptions) -> Result<Self, TomlError>
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let mut bytes = Vec::new();
        for chunk in chunks {
            let chunk = chunk.as_ref();
            if bytes
                .len()
                .checked_add(chunk.len())
                .is_none_or(|length| length > options.limits.max_input_bytes)
            {
                return Err(TomlError::at_zero(TomlErrorKind::ResourceLimit));
            }
            bytes.extend_from_slice(chunk);
        }
        Self::from_bytes(&bytes, options)
    }

    fn from_value(value: TomlValue, options: TomlOptions) -> Result<Self, TomlError> {
        let events = events_for_value(&value)?;
        let mut common_events = Vec::new();
        common_events_for_value(&value, &mut common_events)?;
        if events.len() > options.limits.max_nodes.saturating_mul(4) {
            return Err(TomlError::at_zero(TomlErrorKind::ResourceLimit));
        }
        Ok(Self {
            events,
            index: 0,
            common_events,
            common_index: 0,
            options,
            eof_returned: false,
            finished: false,
            terminal: None,
        })
    }

    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<TomlEvent>, TomlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished || self.eof_returned {
            return Err(TomlError::at_zero(TomlErrorKind::Closed));
        }
        if let Some(event) = self.events.get(self.index).cloned() {
            self.index += 1;
            return Ok(Some(event));
        }
        self.eof_returned = true;
        Ok(None)
    }

    pub fn own(&mut self, event: TomlEvent) -> Result<TomlEvent, TomlError> {
        if self.finished || self.eof_returned {
            return Err(TomlError::at_zero(TomlErrorKind::Closed));
        }
        if let TomlEvent::Scalar(TomlScalar::Text(text)) = &event
            && text.len() > self.options.limits.max_scalar_bytes
        {
            return self.fail(TomlErrorKind::ScalarLimit);
        }
        Ok(event)
    }

    pub fn finish(&mut self) -> Result<(), TomlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished || self.eof_returned {
            return Err(TomlError::at_zero(TomlErrorKind::Closed));
        }
        if self.index != self.events.len() {
            return self.fail(TomlErrorKind::UnexpectedToken);
        }
        self.finished = true;
        self.eof_returned = true;
        Ok(())
    }

    fn next_common(&mut self) -> Result<Option<Event>, TomlError> {
        if let Some(event) = self.common_events.get(self.common_index).cloned() {
            self.common_index += 1;
            Ok(Some(event))
        } else {
            Ok(None)
        }
    }

    fn peek_common(&self) -> Option<Event> {
        self.common_events.get(self.common_index).cloned()
    }

    fn fail<T>(&mut self, kind: TomlErrorKind) -> Result<T, TomlError> {
        let error = TomlError::at_zero(kind);
        self.terminal = Some(error.clone());
        Err(error)
    }

    fn serialization_limits(&self) -> serialization::Limits {
        serialization::Limits {
            max_depth: self.options.limits.max_depth,
            max_events: self.options.limits.max_nodes,
            max_bytes: self.options.limits.max_input_bytes,
            max_container_items: self.options.limits.max_array_elements,
        }
    }
}

enum ValueBuilder {
    Array(Vec<TomlValue>),
    Table(Vec<TomlMember>),
}

fn scalar_from_event(scalar: TomlScalar) -> TomlValue {
    match scalar {
        TomlScalar::Bool(value) => TomlValue::Bool(value),
        TomlScalar::Int(value) => TomlValue::Int(value),
        TomlScalar::UInt(value) => TomlValue::UInt(value),
        TomlScalar::Float(value) => TomlValue::Float(value),
        TomlScalar::Text(value) => TomlValue::Text(value),
        TomlScalar::OffsetDateTime(value) => TomlValue::OffsetDateTime(value),
        TomlScalar::LocalDateTime(value) => TomlValue::LocalDateTime(value),
        TomlScalar::LocalDate(value) => TomlValue::LocalDate(value),
        TomlScalar::LocalTime(value) => TomlValue::LocalTime(value),
    }
}

fn append_array_table(root: &mut TomlValue, path: &[String]) -> Result<(), TomlErrorKind> {
    if path.is_empty() {
        return Err(TomlErrorKind::InvalidTableArray);
    }
    let parent = table_mut_at(root, &path[..path.len() - 1], true)?;
    let key = &path[path.len() - 1];
    if let Some(index) = member_index(parent, key) {
        match &mut parent[index].value {
            TomlValue::Array(rows)
                if rows
                    .iter()
                    .all(|value| matches!(value, TomlValue::Table(_))) =>
            {
                rows.push(TomlValue::Table(Vec::new()));
            }
            _ => return Err(TomlErrorKind::InvalidTableArray),
        }
    } else {
        parent.push(TomlMember {
            key: key.clone(),
            value: TomlValue::Array(vec![TomlValue::Table(Vec::new())]),
        });
    }
    Ok(())
}

fn attach_built_value(
    root: &mut TomlValue,
    pending: &mut Option<Vec<String>>,
    builders: &mut [ValueBuilder],
    value: TomlValue,
) -> Result<(), TomlErrorKind> {
    if let Some(builder) = builders.last_mut() {
        match builder {
            ValueBuilder::Array(values) => values.push(value),
            ValueBuilder::Table(members) => {
                let path = pending.take().ok_or(TomlErrorKind::UnexpectedToken)?;
                insert_table_value(members, &path, value)?;
            }
        }
        return Ok(());
    }
    let path = pending.take().ok_or(TomlErrorKind::UnexpectedToken)?;
    assign_value(root, &path, value)
}

fn own_events_to_value(events: &[TomlEvent]) -> Result<TomlValue, TomlError> {
    let mut root = TomlValue::Table(Vec::new());
    let mut current = Vec::<String>::new();
    let mut table_stack = Vec::<Vec<String>>::new();
    let mut builders = Vec::<ValueBuilder>::new();
    let mut builder_targets = Vec::<Option<Vec<String>>>::new();
    let mut pending = None;
    let mut tables = HashSet::<Vec<String>>::new();
    let mut started = false;
    let mut ended = false;
    for event in events {
        match event {
            TomlEvent::StreamStart if !started => started = true,
            TomlEvent::StreamStart => {
                return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
            }
            TomlEvent::StreamEnd if started && !ended => ended = true,
            TomlEvent::StreamEnd => return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken)),
            TomlEvent::TableStart(path) => {
                if ended || !started {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                if path.is_empty() || !tables.insert(path.clone()) {
                    return Err(TomlError::at_zero(TomlErrorKind::DuplicateTable));
                }
                table_mut_at(&mut root, path, true)
                    .map_err(|kind| TomlError::at_zero(kind).with_path(path))?;
                table_stack.push(current);
                current = path.clone();
            }
            TomlEvent::ArrayTableStart(path) => {
                if ended || !started {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                append_array_table(&mut root, path)
                    .map_err(|kind| TomlError::at_zero(kind).with_path(path))?;
                table_stack.push(current);
                current = path.clone();
            }
            TomlEvent::TableEnd => {
                if ended {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                current = table_stack
                    .pop()
                    .ok_or_else(|| TomlError::at_zero(TomlErrorKind::UnexpectedToken))?;
            }
            TomlEvent::Key(path) => {
                if ended || pending.is_some() {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                pending = Some(path.clone());
            }
            TomlEvent::Scalar(scalar) => {
                if ended {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                attach_built_value(
                    &mut root,
                    &mut pending,
                    &mut builders,
                    scalar_from_event(scalar.clone()),
                )
                .map_err(TomlError::at_zero)?;
            }
            TomlEvent::ArrayStart => {
                if ended {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                builder_targets.push(pending.take());
                builders.push(ValueBuilder::Array(Vec::new()));
            }
            TomlEvent::ArrayEnd => {
                let Some(ValueBuilder::Array(values)) = builders.pop() else {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                };
                let Some(target) = builder_targets.pop() else {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                };
                let mut target = target;
                attach_built_value(
                    &mut root,
                    &mut target,
                    &mut builders,
                    TomlValue::Array(values),
                )
                .map_err(TomlError::at_zero)?;
            }
            TomlEvent::InlineTableStart => {
                if ended {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                }
                builder_targets.push(pending.take());
                builders.push(ValueBuilder::Table(Vec::new()));
            }
            TomlEvent::InlineTableEnd => {
                let Some(ValueBuilder::Table(members)) = builders.pop() else {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                };
                let Some(target) = builder_targets.pop() else {
                    return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                };
                let mut target = target;
                attach_built_value(
                    &mut root,
                    &mut target,
                    &mut builders,
                    TomlValue::Table(members),
                )
                .map_err(TomlError::at_zero)?;
            }
        }
    }
    if !started
        || !ended
        || pending.is_some()
        || !builders.is_empty()
        || !builder_targets.is_empty()
        || !table_stack.is_empty()
    {
        return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
    }
    Ok(root)
}

fn common_events_to_value(events: &[Event]) -> Result<TomlValue, TomlError> {
    fn parse_one(events: &[Event], index: &mut usize) -> Result<TomlValue, TomlError> {
        let Some(event) = events.get(*index).cloned() else {
            return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
        };
        *index += 1;
        match event {
            Event::StartMap(_) | Event::StartRecord { .. } => {
                let mut members = Vec::new();
                loop {
                    let Some(next) = events.get(*index).cloned() else {
                        return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken));
                    };
                    match next {
                        Event::EndMap | Event::EndRecord => {
                            *index += 1;
                            break;
                        }
                        Event::MapKey => {
                            *index += 1;
                            let Some(Event::String(key)) = events.get(*index).cloned() else {
                                return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch));
                            };
                            *index += 1;
                            let value = parse_one(events, index)?;
                            insert_table_value(&mut members, &[key], value)
                                .map_err(TomlError::at_zero)?;
                        }
                        Event::Field(key) => {
                            *index += 1;
                            let value = parse_one(events, index)?;
                            insert_table_value(&mut members, &[key], value)
                                .map_err(TomlError::at_zero)?;
                        }
                        _ => return Err(TomlError::at_zero(TomlErrorKind::UnexpectedToken)),
                    }
                }
                Ok(TomlValue::Table(members))
            }
            Event::StartArray(_) => {
                let mut values = Vec::new();
                loop {
                    if matches!(events.get(*index), Some(Event::EndArray)) {
                        *index += 1;
                        break;
                    }
                    values.push(parse_one(events, index)?);
                }
                Ok(TomlValue::Array(values))
            }
            Event::Bool(value) => Ok(TomlValue::Bool(value)),
            Event::Int(value) => i64::try_from(value)
                .map(TomlValue::Int)
                .map_err(|_| TomlError::at_zero(TomlErrorKind::IntegerOutOfRange)),
            Event::UInt(value) => u64::try_from(value)
                .map(TomlValue::UInt)
                .map_err(|_| TomlError::at_zero(TomlErrorKind::IntegerOutOfRange)),
            Event::Float(value) => Ok(TomlValue::Float(value)),
            Event::Float32(value) => Ok(TomlValue::Float(f32::from_bits(value) as f64)),
            Event::Float64(value) => Ok(TomlValue::Float(f64::from_bits(value))),
            Event::String(value) => Ok(TomlValue::Text(value)),
            Event::Null
            | Event::Bytes(_)
            | Event::MapKey
            | Event::EndArray
            | Event::EndMap
            | Event::StartEnum { .. }
            | Event::EndEnum
            | Event::Field(_)
            | Event::EndRecord => Err(TomlError::at_zero(TomlErrorKind::TypeMismatch)),
        }
    }

    let mut index = 0;
    let value = parse_one(events, &mut index)?;
    if index != events.len() {
        return Err(TomlError::at_zero(TomlErrorKind::TrailingInput));
    }
    if !matches!(value, TomlValue::Table(_)) {
        return Err(TomlError::at_zero(TomlErrorKind::TypeMismatch));
    }
    Ok(value)
}

impl Encoder<TomlCodec, TomlError> for TomlWriter {
    fn write_event(&mut self, event: Event) -> Result<(), TomlError> {
        if self.finished || self.own_events {
            return self.fail(TomlErrorKind::Closed);
        }
        if self.common_events.len() >= self.options.limits.max_nodes.saturating_mul(4) {
            return self.fail(TomlErrorKind::ResourceLimit);
        }
        self.common_events.push(event);
        Ok(())
    }
}

impl Decoder<TomlCodec, TomlError> for TomlReader {
    fn limits(&self) -> serialization::Limits {
        self.serialization_limits()
    }

    fn peek_event(&mut self) -> Result<Option<Event>, TomlError> {
        Ok(self.peek_common())
    }

    fn next(&mut self) -> Result<Option<Event>, TomlError> {
        self.next_common()
    }

    fn reject(&mut self, error: SerializationError) -> TomlError {
        error.into()
    }
}

impl From<TomlValue> for serialization::Value {
    fn from(value: TomlValue) -> Self {
        match value {
            TomlValue::Null => Self::Null,
            TomlValue::Bool(value) => Self::Bool(value),
            TomlValue::Int(value) => Self::Int(value),
            TomlValue::UInt(value) => Self::UInt(value),
            TomlValue::Float(value) => Self::Float64(value.to_bits()),
            TomlValue::Text(value) => Self::String(value),
            TomlValue::OffsetDateTime(value) => Self::String(format_offset_datetime(&value)),
            TomlValue::LocalDateTime(value) => Self::String(format_datetime(&value)),
            TomlValue::LocalDate(value) => Self::String(format_date(&value)),
            TomlValue::LocalTime(value) => Self::String(format_time(&value)),
            TomlValue::Array(values) => Self::Array(values.into_iter().map(Self::from).collect()),
            TomlValue::Table(members) => Self::Object(
                members
                    .into_iter()
                    .map(|member| (member.key, Self::from(member.value)))
                    .collect(),
            ),
        }
    }
}

impl TryFrom<serialization::Value> for TomlValue {
    type Error = TomlError;

    fn try_from(value: serialization::Value) -> Result<Self, Self::Error> {
        match value {
            serialization::Value::Null => Ok(Self::Null),
            serialization::Value::Bool(value) => Ok(Self::Bool(value)),
            serialization::Value::Int(value) => Ok(Self::Int(value)),
            serialization::Value::UInt(value) => Ok(Self::UInt(value)),
            serialization::Value::Float32(value) => Ok(Self::Float(f32::from_bits(value) as f64)),
            serialization::Value::Float64(value) => Ok(Self::Float(f64::from_bits(value))),
            serialization::Value::Number(value) => parse_number(&value).map_err(TomlError::at_zero),
            serialization::Value::String(value) => Ok(Self::Text(value)),
            serialization::Value::Array(values) => Ok(Self::Array(
                values
                    .into_iter()
                    .map(Self::try_from)
                    .collect::<Result<_, _>>()?,
            )),
            serialization::Value::Object(members) => Ok(Self::Table(
                members
                    .into_iter()
                    .map(|(key, value)| {
                        Ok(TomlMember {
                            key,
                            value: Self::try_from(value)?,
                        })
                    })
                    .collect::<Result<_, TomlError>>()?,
            )),
            serialization::Value::Bytes(_)
            | serialization::Value::Map(_)
            | serialization::Value::Extension { .. } => {
                Err(TomlError::at_zero(TomlErrorKind::TypeMismatch))
            }
        }
    }
}

pub fn encode_static<T: Encode<TomlCodec>>(
    value: &T,
    options: TomlOptions,
) -> Result<Vec<u8>, TomlError> {
    let mut writer = TomlWriter::to_writer(options)?;
    value.encode(&mut writer)?;
    writer.finish()
}

pub fn decode_static<T: Decode<TomlCodec>>(
    input: &[u8],
    options: TomlOptions,
) -> Result<T, TomlError> {
    let mut reader = TomlReader::from_bytes(input, options)?;
    let value = T::decode(&mut reader)?;
    while reader.next_common()?.is_some() {}
    while reader.index < reader.events.len() {
        let _ = reader.next()?;
    }
    reader.finish()?;
    Ok(value)
}

/// Event writer used by the hosted boundary.  It buffers until finish so a
/// malformed stream never publishes partial bytes.
#[derive(Debug)]
pub struct TomlWriter {
    options: TomlOptions,
    events: Vec<TomlEvent>,
    common_events: Vec<Event>,
    own_events: bool,
    finished: bool,
    terminal: Option<TomlError>,
}

impl TomlWriter {
    pub fn to_writer(options: TomlOptions) -> Result<Self, TomlError> {
        if !options.limits.valid() {
            return Err(TomlError::at_zero(TomlErrorKind::InvalidLimit));
        }
        Ok(Self {
            options,
            events: Vec::new(),
            common_events: Vec::new(),
            own_events: false,
            finished: false,
            terminal: None,
        })
    }

    pub fn write(&mut self, event: TomlEvent) -> Result<(), TomlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished || !self.common_events.is_empty() {
            return self.fail(TomlErrorKind::Closed);
        }
        if self.events.len() >= self.options.limits.max_nodes.saturating_mul(4) {
            return self.fail(TomlErrorKind::ResourceLimit);
        }
        self.own_events = true;
        self.events.push(event);
        Ok(())
    }

    pub fn finish(&mut self) -> Result<Vec<u8>, TomlError> {
        if let Some(error) = &self.terminal {
            return Err(error.clone());
        }
        if self.finished {
            return Err(TomlError::at_zero(TomlErrorKind::Closed));
        }
        let value = if self.own_events {
            match own_events_to_value(&self.events) {
                Ok(value) => value,
                Err(error) => {
                    self.terminal = Some(error.clone());
                    return Err(error);
                }
            }
        } else if !self.common_events.is_empty() {
            match common_events_to_value(&self.common_events) {
                Ok(value) => value,
                Err(error) => {
                    self.terminal = Some(error.clone());
                    return Err(error);
                }
            }
        } else {
            return self.fail(TomlErrorKind::UnexpectedToken);
        };
        let output = match encode(&value, self.options) {
            Ok(output) => output,
            Err(error) => {
                self.terminal = Some(error.clone());
                return Err(error);
            }
        };
        self.finished = true;
        Ok(output)
    }

    fn fail<T>(&mut self, kind: TomlErrorKind) -> Result<T, TomlError> {
        let error = TomlError::at_zero(kind);
        self.terminal = Some(error.clone());
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io;

    fn parse_default(input: &[u8]) -> Result<TomlValue, TomlError> {
        parse(input, TomlOptions::default())
    }

    #[test]
    fn parses_toml_11_scalars_strings_and_temporal_values() {
        let value = parse_default(
            br##"
title = "Tondo"
enabled = true
hex = 0xDEAD_BEEF
large = 9223372036854775808
float = 1.25e+2
special = -inf
basic = """line
  continued"""
literal = '''C:\path\file'''
date = 1979-05-27
time = 07:32
local = 1979-05-27T07:32:00.999999999
offset = 1979-05-27 07:32:00-07:00
"quoted key" = "ok"
"# not a comment" = "text"
"##,
        )
        .unwrap();
        let TomlValue::Table(members) = value else {
            panic!("root table")
        };
        assert!(members.iter().any(|member| member.key == "title"));
        assert!(members.iter().any(|member| member.key == "large"
            && member.value == TomlValue::UInt(9_223_372_036_854_775_808)));
        assert!(
            members
                .iter()
                .any(|member| member.key == "date"
                    && matches!(member.value, TomlValue::LocalDate(_)))
        );
        assert!(members.iter().any(|member| member.key == "offset"
            && matches!(member.value, TomlValue::OffsetDateTime(value) if value.offset_minutes == -420)));
    }

    #[test]
    fn tables_arrays_of_tables_and_inline_tables_are_ordered() {
        let value = parse_default(
            br#"
name = "root"
[owner]
name = "Tom"
[[products]]
name = "Hammer"
details = { sku = "A", price = 10 }
[[products]]
name = "Nail"
[products.details]
sku = "B"
"#,
        )
        .unwrap();
        let TomlValue::Table(root) = value else {
            panic!("root")
        };
        assert_eq!(root[0].key, "name");
        let owner = root.iter().find(|member| member.key == "owner").unwrap();
        assert!(matches!(owner.value, TomlValue::Table(_)));
        let products = root.iter().find(|member| member.key == "products").unwrap();
        let TomlValue::Array(rows) = &products.value else {
            panic!("array of tables")
        };
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], TomlValue::Table(_)));
        assert!(matches!(rows[1], TomlValue::Table(_)));
    }

    #[test]
    fn duplicate_and_collision_errors_have_paths_and_spans() {
        let duplicate = parse_default(b"a = 1\na = 2\n").unwrap_err();
        assert_eq!(duplicate.kind, TomlErrorKind::DuplicateKey);
        assert_eq!(duplicate.path, vec![TomlPathSegment::Key("a".into())]);
        assert!(duplicate.span.start_offset > 0);
        assert_eq!(
            parse_default(b"a = { x = 1 }\na.y = 2\n").unwrap_err().kind,
            TomlErrorKind::InlineTableExtension
        );
        assert_eq!(
            parse_default(b"[a]\nx = 1\n[a]\ny = 2\n").unwrap_err().kind,
            TomlErrorKind::DuplicateTable
        );
        assert_eq!(
            parse_default(b"a = 1\n[a]\n").unwrap_err().kind,
            TomlErrorKind::TableAfterValue
        );
    }

    #[test]
    fn invalid_numbers_dates_escapes_and_limits_are_rejected_atomically() {
        assert_eq!(
            parse_default(b"bad = 01\n").unwrap_err().kind,
            TomlErrorKind::InvalidNumber
        );
        for token in [
            b"bad = 1_.0\n".as_slice(),
            b"bad = 1._0\n".as_slice(),
            b"bad = 1e\n".as_slice(),
            b"bad = .5\n".as_slice(),
        ] {
            assert_eq!(
                parse(token, TomlOptions::default()).unwrap_err().kind,
                TomlErrorKind::InvalidNumber
            );
        }
        assert_eq!(
            parse_default(b"bad = 2020-02-30\n").unwrap_err().kind,
            TomlErrorKind::InvalidDateTime
        );
        assert_eq!(
            parse_default(b"bad = 12:00:00.1234567890\n")
                .unwrap_err()
                .kind,
            TomlErrorKind::DateTimePrecision
        );
        assert_eq!(
            parse_default(br#"bad = "\q""#).unwrap_err().kind,
            TomlErrorKind::InvalidEscape
        );
        let limits = TomlOptions::create(TomlLimits {
            max_array_elements: 1,
            ..TomlLimits::default()
        });
        assert_eq!(
            parse(b"a = [1, 2]\n", limits).unwrap_err().kind,
            TomlErrorKind::ArrayLimit
        );
        let rows = TomlOptions::create(TomlLimits {
            max_array_table_rows: 1,
            ..TomlLimits::default()
        });
        assert_eq!(
            parse(b"[[a]]\nx = 1\n[[a]]\nx = 2\n", rows)
                .unwrap_err()
                .kind,
            TomlErrorKind::TableLimit
        );
        let invalid = TomlLimits::create(0, 1, 1, 1, 1, 1, 1, 1, 1, 1);
        assert_eq!(invalid.unwrap_err().kind, TomlErrorKind::InvalidLimit);
        assert_eq!(
            parse(&[0xff], TomlOptions::default()).unwrap_err().kind,
            TomlErrorKind::InvalidUtf8
        );
    }

    #[test]
    fn canonical_encoding_round_trips_and_sorts_keys() {
        let value = TomlValue::Table(vec![
            TomlMember {
                key: "z".into(),
                value: TomlValue::Int(1),
            },
            TomlMember {
                key: "a".into(),
                value: TomlValue::Table(vec![TomlMember {
                    key: "text".into(),
                    value: TomlValue::Text("a\nb".into()),
                }]),
            },
        ]);
        let encoded = encode_canonical(&value, TomlLimits::default()).unwrap();
        let text = String::from_utf8(encoded.clone()).unwrap();
        assert!(text.starts_with("z = 1\n"));
        assert!(text.contains("[a]\n"));
        assert_eq!(parse_default(&encoded).unwrap(), value);
    }

    #[test]
    fn reader_chunking_events_and_terminal_lifecycle_are_stable() {
        let input = b"a = [1, true]\n[b]\nc = \"x\"\n";
        let mut contiguous = TomlReader::from_bytes(input, TomlOptions::default()).unwrap();
        let mut one_byte =
            TomlReader::from_chunks(input.iter().map(|byte| vec![*byte]), TomlOptions::default())
                .unwrap();
        let mut left = Vec::new();
        let mut right = Vec::new();
        while let Some(event) = contiguous.next().unwrap() {
            left.push(event);
        }
        while let Some(event) = one_byte.next().unwrap() {
            right.push(event);
        }
        assert_eq!(left, right);
        assert!(matches!(left.first(), Some(TomlEvent::StreamStart)));
        assert!(matches!(left.last(), Some(TomlEvent::StreamEnd)));
        contiguous.finish().unwrap_err();
        assert_eq!(contiguous.next().unwrap_err().kind, TomlErrorKind::Closed);
        let mut finished = TomlReader::from_bytes(b"", TomlOptions::default()).unwrap();
        while finished.next().unwrap().is_some() {}
        assert_eq!(finished.finish().unwrap_err().kind, TomlErrorKind::Closed);
    }

    #[test]
    fn writer_and_common_typed_protocol_reject_partial_streams() {
        let mut writer = TomlWriter::to_writer(TomlOptions::default()).unwrap();
        writer.write(TomlEvent::StreamStart).unwrap();
        writer.write(TomlEvent::Key(vec!["a".into()])).unwrap();
        writer.write(TomlEvent::Scalar(TomlScalar::Int(2))).unwrap();
        writer.write(TomlEvent::StreamEnd).unwrap();
        assert_eq!(writer.finish().unwrap(), b"a = 2\n");
        assert_eq!(writer.finish().unwrap_err().kind, TomlErrorKind::Closed);

        let mut broken = TomlWriter::to_writer(TomlOptions::default()).unwrap();
        broken.write(TomlEvent::StreamStart).unwrap();
        broken.write(TomlEvent::Key(vec!["a".into()])).unwrap();
        assert_eq!(
            broken.finish().unwrap_err().kind,
            TomlErrorKind::UnexpectedToken
        );

        let map = BTreeMap::from([("x".to_owned(), 4_i64)]);
        let bytes = encode_static(&map, TomlOptions::default()).unwrap();
        assert_eq!(
            decode_static::<BTreeMap<String, i64>>(&bytes, TomlOptions::default()).unwrap(),
            map
        );
    }

    #[test]
    fn reader_io_and_view_keep_contract_boundaries() {
        let mut reader =
            TomlReader::from_reader(io::Cursor::new(b"a = 1\n"), TomlOptions::default()).unwrap();
        assert!(matches!(
            reader.next().unwrap(),
            Some(TomlEvent::StreamStart)
        ));
        let view = parse_view(b"a = 1\n", TomlOptions::default()).unwrap();
        assert_eq!(view.bytes(), b"a = 1\n");
        assert_eq!(
            view.clone_value().unwrap(),
            parse_default(b"a = 1\n").unwrap()
        );
        let error = TomlReader::from_reader(ErrorReader, TomlOptions::default()).unwrap_err();
        assert!(matches!(error.kind, TomlErrorKind::Io(_)));
    }

    fn assert_kind(input: &[u8], options: TomlOptions, expected: TomlErrorKind) {
        assert_eq!(parse(input, options).unwrap_err().kind, expected);
    }

    fn table_member<'a>(value: &'a TomlValue, key: &str) -> &'a TomlValue {
        let TomlValue::Table(members) = value else {
            panic!("expected table")
        };
        &members
            .iter()
            .find(|member| member.key == key)
            .unwrap_or_else(|| panic!("missing key {key}"))
            .value
    }

    #[test]
    fn syntax_matrix_covers_comments_keys_strings_arrays_and_tables() {
        let value = parse_default(
            br#"
# comments and CRLF are discarded
bare-key = true
"quoted key" = "a,b"
'literal-key' = 'C:\tmp\file'
escaped = "\b\t\n\f\r\e\"\x41\u0042\U00000043\\"
multiline = """first
second \
  third"""
literal-multiline = '''first
second'''
values = [
  1,
  0x10,
  { "x y" = "z", nested.key = 4, },
  [true, false,],
]
[owner]
name = "Tom"
[[owner.workers]]
name = "one"
[owner.workers.meta]
active = true
[[owner.workers]]
name = "two"
"#,
        )
        .unwrap();
        assert_eq!(table_member(&value, "bare-key"), &TomlValue::Bool(true));
        assert_eq!(
            table_member(&value, "quoted key"),
            &TomlValue::Text("a,b".into())
        );
        assert_eq!(
            table_member(&value, "literal-key"),
            &TomlValue::Text(r"C:\tmp\file".into())
        );
        assert_eq!(
            table_member(&value, "escaped"),
            &TomlValue::Text("\u{8}\t\n\u{c}\r\u{1b}\"ABC\\".into())
        );
        assert_eq!(
            table_member(&value, "multiline"),
            &TomlValue::Text("first\nsecond third".into())
        );
        assert_eq!(
            table_member(&value, "literal-multiline"),
            &TomlValue::Text("first\nsecond".into())
        );
        let TomlValue::Array(values) = table_member(&value, "values") else {
            panic!("values array")
        };
        assert_eq!(values[0], TomlValue::Int(1));
        assert_eq!(values[1], TomlValue::Int(16));
        assert!(matches!(values[2], TomlValue::Table(_)));
        assert!(matches!(values[3], TomlValue::Array(_)));
        let owner = table_member(&value, "owner");
        let workers = table_member(owner, "workers");
        assert!(matches!(workers, TomlValue::Array(rows) if rows.len() == 2));
    }

    #[test]
    fn numeric_and_temporal_boundaries_preserve_lossless_values() {
        let value = parse_default(
            br#"
decimal = +1_000
negative = -9223372036854775808
unsigned = 18446744073709551615
binary = 0b1010_0011
octal = 0o7_5
hex = 0xDEAD_BEEF
fraction = 0.125
exponent = -1.5e+2
positive_inf = +inf
negative_inf = -inf
not_a_number = nan
date = 1979-05-27
time = 07:32
precise = 07:32:00.123456789
local = 1979-05-27T07:32:00.1
space = 1979-05-27 07:32:00
utc = 1979-05-27T07:32:00Z
offset = 1979-05-27T07:32:00-07:30
"#,
        )
        .unwrap();
        assert_eq!(table_member(&value, "decimal"), &TomlValue::Int(1_000));
        assert_eq!(table_member(&value, "negative"), &TomlValue::Int(i64::MIN));
        assert_eq!(table_member(&value, "unsigned"), &TomlValue::UInt(u64::MAX));
        assert_eq!(table_member(&value, "binary"), &TomlValue::Int(0xa3));
        assert_eq!(table_member(&value, "octal"), &TomlValue::Int(61));
        assert_eq!(table_member(&value, "hex"), &TomlValue::Int(0xdead_beef));
        assert_eq!(table_member(&value, "fraction"), &TomlValue::Float(0.125));
        assert_eq!(table_member(&value, "exponent"), &TomlValue::Float(-150.0));
        assert!(matches!(
            table_member(&value, "positive_inf"),
            TomlValue::Float(value) if value.is_infinite() && value.is_sign_positive()
        ));
        assert!(matches!(
            table_member(&value, "negative_inf"),
            TomlValue::Float(value) if value.is_infinite() && value.is_sign_negative()
        ));
        assert!(
            matches!(table_member(&value, "not_a_number"), TomlValue::Float(value) if value.is_nan())
        );
        assert!(matches!(
            table_member(&value, "date"),
            TomlValue::LocalDate(TomlDate {
                year: 1979,
                month: 5,
                day: 27
            })
        ));
        assert_eq!(
            table_member(&value, "time"),
            &TomlValue::LocalTime(TomlTime {
                hour: 7,
                minute: 32,
                second: 0,
                nanosecond: 0,
            })
        );
        assert!(matches!(
            table_member(&value, "precise"),
            TomlValue::LocalTime(TomlTime {
                nanosecond: 123_456_789,
                ..
            })
        ));
        assert!(matches!(
            table_member(&value, "local"),
            TomlValue::LocalDateTime(_)
        ));
        assert!(matches!(
            table_member(&value, "space"),
            TomlValue::LocalDateTime(_)
        ));
        assert!(matches!(
            table_member(&value, "utc"),
            TomlValue::OffsetDateTime(TomlOffsetDateTime {
                offset_minutes: 0,
                ..
            })
        ));
        assert!(matches!(
            table_member(&value, "offset"),
            TomlValue::OffsetDateTime(TomlOffsetDateTime {
                offset_minutes: -450,
                ..
            })
        ));
    }

    #[test]
    fn malformed_syntax_is_atomic_and_reports_stable_locations() {
        for (input, expected) in [
            (b"[broken\n".as_slice(), TomlErrorKind::InvalidTable),
            (b"[[broken]\n".as_slice(), TomlErrorKind::InvalidTableArray),
            (b"a =\n".as_slice(), TomlErrorKind::MissingValue),
            (b"a 1\n".as_slice(), TomlErrorKind::UnexpectedToken),
            (b"a = 1 trailing\n".as_slice(), TomlErrorKind::InvalidNumber),
            (b"a = [1\n".as_slice(), TomlErrorKind::InvalidArray),
            (b"a = { x = 1\n".as_slice(), TomlErrorKind::InvalidArray),
            (
                b"a = \"unterminated\n".as_slice(),
                TomlErrorKind::InvalidString,
            ),
            (br#"a = "\q""#.as_slice(), TomlErrorKind::InvalidEscape),
            (b"a = 01\n".as_slice(), TomlErrorKind::InvalidNumber),
            (
                b"a = 18446744073709551616\n".as_slice(),
                TomlErrorKind::IntegerOutOfRange,
            ),
            (
                b"a = 2020-02-30\n".as_slice(),
                TomlErrorKind::InvalidDateTime,
            ),
            (
                b"a = 12:00:00.1234567890\n".as_slice(),
                TomlErrorKind::DateTimePrecision,
            ),
            (b"a = [1,,2]\n".as_slice(), TomlErrorKind::InvalidNumber),
            (b"[a] trailing\n".as_slice(), TomlErrorKind::TrailingInput),
            (b"[[a]] trailing\n".as_slice(), TomlErrorKind::TrailingInput),
        ] {
            assert_kind(input, TomlOptions::default(), expected);
        }
        let duplicate = parse_default(b"first = 1\nfirst = 2\n").unwrap_err();
        assert_eq!(duplicate.kind, TomlErrorKind::DuplicateKey);
        assert_eq!(duplicate.path, vec![TomlPathSegment::Key("first".into())]);
        assert_eq!(duplicate.span.start_line, 2);
        assert_eq!(duplicate.span.start_column, 1);
        assert!(duplicate.span.end_offset > duplicate.span.start_offset);
        assert_eq!(
            parse_default(b"inline = { value = 1, }\ninline.value = 2\n")
                .unwrap_err()
                .kind,
            TomlErrorKind::InlineTableExtension
        );
        assert_eq!(
            parse_default(b"[a]\nx = 1\n[a]\ny = 2\n").unwrap_err().kind,
            TomlErrorKind::DuplicateTable
        );
        assert_eq!(
            parse_default(b"a = 1\n[a]\n").unwrap_err().kind,
            TomlErrorKind::TableAfterValue
        );
    }

    #[test]
    fn every_materialisation_limit_rejects_without_partial_values() {
        let cases = [
            (
                b"a = 1\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_input_bytes: 2,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::ResourceLimit,
            ),
            (
                b"a = [1]\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_depth: 1,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::DepthLimit,
            ),
            (
                b"a = 1\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_nodes: 1,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::NodeLimit,
            ),
            (
                b"[a]\nx = 1\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_tables: 1,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::TableLimit,
            ),
            (
                b"a = [1, 2]\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_array_elements: 1,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::ArrayLimit,
            ),
            (
                b"long = 1\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_key_bytes: 2,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::KeyLimit,
            ),
            (
                b"a.b = 1\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_path_segments: 1,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::KeyLimit,
            ),
            (
                b"value = \"long\"\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_string_bytes: 2,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::StringLimit,
            ),
            (
                b"value = \"long\"\n".as_slice(),
                TomlOptions::create(TomlLimits {
                    max_scalar_bytes: 2,
                    ..TomlLimits::default()
                }),
                TomlErrorKind::ScalarLimit,
            ),
        ];
        for (input, options, expected) in cases {
            assert_kind(input, options, expected);
        }
        let bad_limits =
            TomlLimits::create(usize::MAX, 1, usize::MAX, usize::MAX, 1, 1, 1, 1, 1, 1);
        assert_eq!(bad_limits.unwrap_err().kind, TomlErrorKind::InvalidLimit);
        assert_kind(
            b"[[rows]]\nx = 1\n[[rows]]\nx = 2\n",
            TomlOptions::create(TomlLimits {
                max_array_table_rows: 1,
                ..TomlLimits::default()
            }),
            TomlErrorKind::TableLimit,
        );
        assert_kind(
            b"a = [1]\n",
            TomlOptions::create(TomlLimits {
                max_nodes: 2,
                ..TomlLimits::default()
            }),
            TomlErrorKind::NodeLimit,
        );
    }

    #[test]
    fn dynamic_encoding_and_validation_cover_all_value_shapes() {
        let value = TomlValue::Table(vec![
            TomlMember {
                key: "z".into(),
                value: TomlValue::Array(vec![
                    TomlValue::Bool(true),
                    TomlValue::Int(-2),
                    TomlValue::UInt(u64::MAX),
                    TomlValue::Float(1.5),
                    TomlValue::Float(f64::INFINITY),
                    TomlValue::Float(f64::NAN),
                    TomlValue::Text("line\n\tquote \" \\".into()),
                    TomlValue::LocalTime(TomlTime {
                        hour: 7,
                        minute: 32,
                        second: 1,
                        nanosecond: 12_300_000,
                    }),
                    TomlValue::Array(Vec::new()),
                    TomlValue::Table(vec![TomlMember {
                        key: "quoted key".into(),
                        value: TomlValue::Text("inline".into()),
                    }]),
                ]),
            },
            TomlMember {
                key: "nested".into(),
                value: TomlValue::Table(vec![TomlMember {
                    key: "empty".into(),
                    value: TomlValue::Table(Vec::new()),
                }]),
            },
            TomlMember {
                key: "rows".into(),
                value: TomlValue::Array(vec![
                    TomlValue::Table(vec![TomlMember {
                        key: "id".into(),
                        value: TomlValue::Int(1),
                    }]),
                    TomlValue::Table(vec![TomlMember {
                        key: "id".into(),
                        value: TomlValue::Int(2),
                    }]),
                ]),
            },
        ]);
        let insertion = encode(&value, TomlOptions::default()).unwrap();
        let canonical = encode_canonical(&value, TomlLimits::default()).unwrap();
        assert_eq!(
            encode(&parse_default(&insertion).unwrap(), TomlOptions::default()).unwrap(),
            insertion
        );
        assert_eq!(
            encode_canonical(&parse_default(&canonical).unwrap(), TomlLimits::default()).unwrap(),
            canonical
        );
        let canonical_text = String::from_utf8(canonical).unwrap();
        assert!(canonical_text.contains("nan"));
        assert!(canonical_text.contains("inf"));
        assert!(canonical_text.contains("[[rows]]"));
        assert!(canonical_text.contains("\"quoted key\""));
        assert_eq!(
            encode(&TomlValue::Int(1), TomlOptions::default())
                .unwrap_err()
                .kind,
            TomlErrorKind::TypeMismatch
        );
        assert_eq!(
            encode(
                &TomlValue::Table(vec![TomlMember {
                    key: "null".into(),
                    value: TomlValue::Null,
                }]),
                TomlOptions::default()
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::TypeMismatch
        );
        let duplicate = TomlValue::Table(vec![
            TomlMember {
                key: "a".into(),
                value: TomlValue::Int(1),
            },
            TomlMember {
                key: "a".into(),
                value: TomlValue::Int(2),
            },
        ]);
        assert_eq!(
            encode(&duplicate, TomlOptions::default()).unwrap_err().kind,
            TomlErrorKind::DuplicateKey
        );
        assert_eq!(
            encode(
                &TomlValue::Table(vec![TomlMember {
                    key: "x".into(),
                    value: TomlValue::Text("long".into()),
                }]),
                TomlOptions::create(TomlLimits {
                    max_string_bytes: 2,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::StringLimit
        );
        assert_eq!(
            encode(
                &TomlValue::Table(vec![TomlMember {
                    key: "x".into(),
                    value: TomlValue::Table(Vec::new()),
                }]),
                TomlOptions::create(TomlLimits {
                    max_depth: 1,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::DepthLimit
        );
    }

    #[test]
    fn own_and_common_event_protocols_round_trip_and_reject_malformed_streams() {
        let value = TomlValue::Table(vec![
            TomlMember {
                key: "flag".into(),
                value: TomlValue::Bool(true),
            },
            TomlMember {
                key: "array".into(),
                value: TomlValue::Array(vec![
                    TomlValue::Int(1),
                    TomlValue::Table(vec![TomlMember {
                        key: "name".into(),
                        value: TomlValue::Text("row".into()),
                    }]),
                ]),
            },
            TomlMember {
                key: "rows".into(),
                value: TomlValue::Array(vec![TomlValue::Table(vec![TomlMember {
                    key: "id".into(),
                    value: TomlValue::Int(1),
                }])]),
            },
            TomlMember {
                key: "when".into(),
                value: TomlValue::OffsetDateTime(TomlOffsetDateTime {
                    local: TomlDateTime {
                        date: TomlDate {
                            year: 2026,
                            month: 9,
                            day: 7,
                        },
                        time: TomlTime {
                            hour: 12,
                            minute: 0,
                            second: 1,
                            nanosecond: 0,
                        },
                    },
                    offset_minutes: 60,
                }),
            },
        ]);
        let own = events_for_value(&value).unwrap();
        assert_eq!(own_events_to_value(&own).unwrap(), value);
        assert_eq!(own.first(), Some(&TomlEvent::StreamStart));
        assert_eq!(own.last(), Some(&TomlEvent::StreamEnd));

        let common = vec![
            Event::StartRecord {
                name: "Root".into(),
                fields: Some(2),
            },
            Event::Field("items".into()),
            Event::StartArray(Some(2)),
            Event::Int(1),
            Event::Float32(1.5_f32.to_bits()),
            Event::EndArray,
            Event::Field("ok".into()),
            Event::Bool(true),
            Event::EndRecord,
        ];
        let common_value = common_events_to_value(&common).unwrap();
        assert_eq!(
            common_value,
            TomlValue::Table(vec![
                TomlMember {
                    key: "items".into(),
                    value: TomlValue::Array(vec![TomlValue::Int(1), TomlValue::Float(1.5),]),
                },
                TomlMember {
                    key: "ok".into(),
                    value: TomlValue::Bool(true),
                },
            ])
        );
        for events in [
            vec![],
            vec![TomlEvent::StreamStart],
            vec![TomlEvent::StreamEnd],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::StreamEnd,
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::Scalar(TomlScalar::Int(1)),
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::Key(vec!["a".into()]),
                TomlEvent::Key(vec!["b".into()]),
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::ArrayEnd,
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::InlineTableEnd,
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::TableEnd,
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::TableStart(Vec::new()),
                TomlEvent::StreamEnd,
            ],
            vec![
                TomlEvent::StreamStart,
                TomlEvent::ArrayTableStart(Vec::new()),
                TomlEvent::StreamEnd,
            ],
        ] {
            assert!(own_events_to_value(&events).is_err(), "events: {events:?}");
        }
        assert_eq!(
            common_events_to_value(&[Event::StartArray(None), Event::EndArray])
                .unwrap_err()
                .kind,
            TomlErrorKind::TypeMismatch
        );
        assert_eq!(
            common_events_to_value(&[Event::Null]).unwrap_err().kind,
            TomlErrorKind::TypeMismatch
        );
        assert_eq!(
            common_events_to_value(&[Event::StartMap(None), Event::MapKey, Event::Int(1)])
                .unwrap_err()
                .kind,
            TomlErrorKind::TypeMismatch
        );
        assert_eq!(
            common_events_to_value(&[
                Event::StartMap(None),
                Event::MapKey,
                Event::String("a".into()),
                Event::Int(1),
                Event::EndMap,
                Event::Bool(true),
            ])
            .unwrap_err()
            .kind,
            TomlErrorKind::TrailingInput
        );
    }

    #[test]
    fn typed_protocol_conversions_and_stream_lifecycle_are_bounded() {
        use crate::serialization::Encoder as _;

        assert_eq!(
            encode_typed(&Some(4_i64), TomlOptions::default())
                .unwrap_err()
                .kind,
            TomlErrorKind::TypeMismatch
        );
        assert_eq!(
            decode_typed::<i64>(b"x = true\n", TomlOptions::default())
                .unwrap_err()
                .kind,
            TomlErrorKind::TypeMismatch
        );

        let mut common_writer = TomlWriter::to_writer(TomlOptions::default()).unwrap();
        common_writer.start_map(Some(1)).unwrap();
        common_writer.map_key().unwrap();
        common_writer.string("x").unwrap();
        common_writer.int(1).unwrap();
        common_writer.end_map().unwrap();
        assert_eq!(common_writer.finish().unwrap(), b"x = 1\n");
        assert_eq!(
            common_writer.finish().unwrap_err().kind,
            TomlErrorKind::Closed
        );
        let mut limited_writer = TomlWriter::to_writer(TomlOptions::create(TomlLimits {
            max_nodes: 1,
            ..TomlLimits::default()
        }))
        .unwrap();
        limited_writer.start_map(None).unwrap();
        limited_writer.map_key().unwrap();
        limited_writer.string("x").unwrap();
        limited_writer.int(1).unwrap();
        assert_eq!(
            limited_writer.write_event(Event::EndMap).unwrap_err().kind,
            TomlErrorKind::ResourceLimit
        );

        let values = [
            TomlValue::Null,
            TomlValue::Bool(true),
            TomlValue::Int(-2),
            TomlValue::UInt(3),
            TomlValue::Float(1.5),
            TomlValue::Text("text".into()),
            TomlValue::Array(vec![TomlValue::Bool(false)]),
            TomlValue::Table(vec![TomlMember {
                key: "k".into(),
                value: TomlValue::UInt(1),
            }]),
        ];
        for value in values {
            let serialized: serialization::Value = value.clone().into();
            assert_eq!(TomlValue::try_from(serialized).unwrap(), value);
        }
        let temporal = TomlValue::OffsetDateTime(TomlOffsetDateTime {
            local: TomlDateTime {
                date: TomlDate {
                    year: 2026,
                    month: 9,
                    day: 7,
                },
                time: TomlTime {
                    hour: 12,
                    minute: 0,
                    second: 0,
                    nanosecond: 0,
                },
            },
            offset_minutes: 0,
        });
        assert_eq!(
            serialization::Value::from(temporal),
            serialization::Value::String("2026-09-07T12:00:00Z".into())
        );
        assert_eq!(
            TomlValue::try_from(serialization::Value::Float32(1.5_f32.to_bits())).unwrap(),
            TomlValue::Float(1.5)
        );
        assert_eq!(
            TomlValue::try_from(serialization::Value::Float64(2.5_f64.to_bits())).unwrap(),
            TomlValue::Float(2.5)
        );
        assert_eq!(
            TomlValue::try_from(serialization::Value::Number("0x10".into())).unwrap(),
            TomlValue::Int(16)
        );
        for value in [
            serialization::Value::Bytes(vec![1]),
            serialization::Value::Map(Vec::new()),
            serialization::Value::Extension {
                type_code: 1,
                payload: vec![],
            },
        ] {
            assert_eq!(
                TomlValue::try_from(value).unwrap_err().kind,
                TomlErrorKind::TypeMismatch
            );
        }

        let mut decoder =
            TomlReader::from_bytes(b"a = [1, true]\n", TomlOptions::default()).unwrap();
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::peek_event(&mut decoder).unwrap(),
            Some(Event::StartMap(Some(1)))
        );
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::limits(&decoder).max_depth,
            TomlLimits::default().max_depth
        );
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::next(&mut decoder).unwrap(),
            Some(Event::StartMap(Some(1)))
        );
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::next(&mut decoder).unwrap(),
            Some(Event::MapKey)
        );
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::next(&mut decoder).unwrap(),
            Some(Event::String("a".into()))
        );
        assert_eq!(
            <TomlReader as Decoder<TomlCodec, TomlError>>::next(&mut decoder).unwrap(),
            Some(Event::StartArray(Some(2)))
        );
        let own_limit = TomlOptions::create(TomlLimits {
            max_scalar_bytes: 1,
            ..TomlLimits::default()
        });
        let mut own_reader = TomlReader::from_bytes(b"a = 1\n", own_limit).unwrap();
        assert_eq!(
            own_reader
                .own(TomlEvent::Scalar(TomlScalar::Text("long".into())))
                .unwrap_err()
                .kind,
            TomlErrorKind::ScalarLimit
        );
        assert_eq!(
            own_reader.next().unwrap_err().kind,
            TomlErrorKind::ScalarLimit
        );
    }

    #[test]
    fn reader_input_limits_and_terminal_errors_are_explicit() {
        assert_eq!(
            TomlReader::from_chunks(
                [b"a = 1\n".as_slice(), b"".as_slice()],
                TomlOptions::create(TomlLimits {
                    max_input_bytes: 4,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::ResourceLimit
        );
        assert_eq!(
            TomlReader::from_reader(
                io::Cursor::new(b"a = 1\n"),
                TomlOptions::create(TomlLimits {
                    max_input_bytes: 2,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::ResourceLimit
        );
        assert_eq!(
            TomlReader::from_bytes(
                b"a = 1\n",
                TomlOptions::create(TomlLimits {
                    max_nodes: 1,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::NodeLimit
        );
        assert_eq!(
            TomlReader::from_bytes(
                b"a = 1\n",
                TomlOptions::create(TomlLimits {
                    max_nodes: 1,
                    max_input_bytes: 64,
                    ..TomlLimits::default()
                })
            )
            .unwrap_err()
            .kind,
            TomlErrorKind::NodeLimit
        );
        let mut broken = TomlWriter::to_writer(TomlOptions::default()).unwrap();
        assert_eq!(
            broken.finish().unwrap_err().kind,
            TomlErrorKind::UnexpectedToken
        );
        assert_eq!(
            broken.finish().unwrap_err().kind,
            TomlErrorKind::UnexpectedToken
        );
        let mut unbalanced = TomlWriter::to_writer(TomlOptions::default()).unwrap();
        unbalanced.write(TomlEvent::StreamStart).unwrap();
        assert_eq!(
            unbalanced.finish().unwrap_err().kind,
            TomlErrorKind::UnexpectedToken
        );
        assert_eq!(
            unbalanced.write(TomlEvent::StreamEnd).unwrap_err().kind,
            TomlErrorKind::UnexpectedToken
        );
    }

    struct ErrorReader;

    impl Read for ErrorReader {
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("boom"))
        }
    }
}
