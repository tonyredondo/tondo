//! Bounded pure structured logging values and scalar record formatting.
//! The host owns capabilities, admission and resource lifetimes. This kernel
//! neither selects a provider nor claims native execution or measured performance.

use crate::civil_time::UtcDateTime;
use crate::encoding::{Base64Options, EncodingLimits};
use crate::serialization::Bytes;
use std::sync::Arc;

/// Closed pure operations shared by signature checking and the host bridge.
/// Fields insertion returns a new value; the ordinary Tondo wrapper performs
/// its single mutation only after successful admission of that value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogOperation {
    FieldsWithField,
    EventCreate,
}
impl LogOperation {
    pub const ALL: [Self; 2] = [Self::FieldsWithField, Self::EventCreate];
    pub const fn name(self) -> &'static str {
        match self {
            Self::FieldsWithField => "std.log.Fields.__withField",
            Self::EventCreate => "std.log.LogEvent.__create",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|operation| operation.name() == name)
    }
    pub fn associated(owner: &str, member: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|operation| {
            operation
                .name()
                .strip_prefix("std.log.")
                .and_then(|name| name.split_once('.'))
                == Some((owner, member))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}
impl LogLevel {
    pub const fn text(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    JsonLines,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogReceipt {
    Accepted,
    Dropped,
    Filtered,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backpressure {
    Block,
    Reject,
    Drop,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogError {
    InvalidTarget,
    InvalidFieldKey,
    DuplicateField,
    InvalidLimit,
    ResourceLimit,
    NonFiniteValue,
    Backpressure,
    Closed,
    Cancelled,
    CapabilityMissing,
    UnsupportedFormat,
    Io,
    Host,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogLimits {
    pub max_event_bytes: usize,
    pub max_fields: usize,
    pub max_depth: usize,
    pub max_field_key_bytes: usize,
    pub max_string_bytes: usize,
    pub max_queue_entries: usize,
}
impl Default for LogLimits {
    fn default() -> Self {
        Self {
            max_event_bytes: 1 << 20,
            max_fields: 64,
            max_depth: 16,
            max_field_key_bytes: 128,
            max_string_bytes: 64 << 10,
            max_queue_entries: 1024,
        }
    }
}
impl LogLimits {
    pub fn create(
        event: usize,
        fields: usize,
        depth: usize,
        key: usize,
        string: usize,
        queue: usize,
    ) -> Result<Self, LogError> {
        if [event, fields, depth, key, string, queue].contains(&0) {
            return Err(LogError::InvalidLimit);
        }
        Ok(Self {
            max_event_bytes: event,
            max_fields: fields,
            max_depth: depth,
            max_field_key_bytes: key,
            max_string_bytes: string,
            max_queue_entries: queue,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogValue {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(Arc<str>),
    Bytes(Arc<[u8]>),
    Array(Arc<[LogValue]>),
    Object(Fields),
    Redacted,
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fields(Arc<[(Arc<str>, LogValue)]>);
impl Fields {
    pub fn empty() -> Self {
        Self::default()
    }
    pub fn count(&self) -> usize {
        self.0.len()
    }
    pub fn get(&self, key: &str) -> Option<&LogValue> {
        self.0
            .iter()
            .find(|(name, _)| name.as_ref() == key)
            .map(|(_, value)| value)
    }
    /// Validate the entire candidate before replacing the immutable backing.
    pub fn put(&mut self, key: String, value: LogValue) -> Result<(), LogError> {
        let limits = LogLimits::default();
        self.check_insertion_key(&key)?;
        let existing = self.0.iter().map(|(key, value)| (key.as_ref(), value));
        validate_values(
            existing.chain(std::iter::once((key.as_str(), &value))),
            limits,
        )?;
        let mut next = Vec::new();
        next.try_reserve_exact(self.count() + 1)
            .map_err(|_| LogError::ResourceLimit)?;
        next.extend(self.0.iter().cloned());
        next.push((key.into(), value));
        self.0 = next.into();
        Ok(())
    }
    /// Refuse an invalid insertion before a bridge copies its new payload.
    pub fn check_insertion_key(&self, key: &str) -> Result<(), LogError> {
        let limits = LogLimits::default();
        validate_key(key, limits)?;
        if self.get(key).is_some() {
            return Err(LogError::DuplicateField);
        }
        if self.count() >= limits.max_fields {
            return Err(LogError::ResourceLimit);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    level: LogLevel,
    target: Arc<str>,
    message: Arc<str>,
    fields: Fields,
    timestamp: Option<UtcDateTime>,
}
impl LogEvent {
    pub fn create(
        level: LogLevel,
        target: String,
        message: String,
        fields: Fields,
        timestamp: Option<UtcDateTime>,
    ) -> Result<Self, LogError> {
        if target.is_empty() {
            return Err(LogError::InvalidTarget);
        }
        let limits = LogLimits::default();
        validate_event_parts(&target, &message, &fields, limits)?;
        Ok(Self {
            level,
            target: target.into(),
            message: message.into(),
            fields,
            timestamp,
        })
    }
    pub const fn level(&self) -> LogLevel {
        self.level
    }
    pub fn target(&self) -> &str {
        &self.target
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn fields(&self) -> &Fields {
        &self.fields
    }
    pub const fn timestamp(&self) -> Option<UtcDateTime> {
        self.timestamp
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoggerOptions {
    minimum_level: LogLevel,
}
impl LoggerOptions {
    pub const fn create(minimum_level: LogLevel) -> Self {
        Self { minimum_level }
    }
    pub fn enabled(self, level: LogLevel) -> bool {
        level >= self.minimum_level
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinkOptions {
    pub format: LogFormat,
    pub backpressure: Backpressure,
    pub capacity: usize,
    pub limits: LogLimits,
}
impl SinkOptions {
    pub fn create(
        format: LogFormat,
        backpressure: Backpressure,
        capacity: usize,
        limits: LogLimits,
    ) -> Result<Self, LogError> {
        LogLimits::create(
            limits.max_event_bytes,
            limits.max_fields,
            limits.max_depth,
            limits.max_field_key_bytes,
            limits.max_string_bytes,
            limits.max_queue_entries,
        )?;
        if capacity == 0 {
            return Err(LogError::InvalidLimit);
        }
        if capacity > limits.max_queue_entries {
            return Err(LogError::ResourceLimit);
        }
        Ok(Self {
            format,
            backpressure,
            capacity,
            limits,
        })
    }
}

fn add_bytes(total: &mut usize, delta: usize, maximum: usize) -> Result<(), LogError> {
    *total = total.checked_add(delta).ok_or(LogError::ResourceLimit)?;
    if *total > maximum {
        return Err(LogError::ResourceLimit);
    }
    Ok(())
}
fn validate_key(key: &str, limits: LogLimits) -> Result<(), LogError> {
    if key.is_empty() || key.chars().any(char::is_control) {
        return Err(LogError::InvalidFieldKey);
    }
    if key.len() > limits.max_field_key_bytes {
        return Err(LogError::ResourceLimit);
    }
    Ok(())
}
/// Traversal retains one iterator per container rather than all sibling values.
fn validate_values<'a>(
    roots: impl Iterator<Item = (&'a str, &'a LogValue)>,
    limits: LogLimits,
) -> Result<usize, LogError> {
    enum Frame<'a> {
        Value(&'a LogValue, usize),
        Array(std::slice::Iter<'a, LogValue>, usize),
        Object(std::slice::Iter<'a, (Arc<str>, LogValue)>, usize),
    }
    let mut total = 0;
    let mut fields = 0usize;
    let mut stack = Vec::new();
    for (key, value) in roots {
        fields = fields.checked_add(1).ok_or(LogError::ResourceLimit)?;
        validate_key(key, limits)?;
        add_bytes(&mut total, key.len(), limits.max_event_bytes)?;
        stack.push(Frame::Value(value, 0));
        while let Some(frame) = stack.pop() {
            match frame {
                Frame::Value(value, depth) => {
                    // Empty strings and containers still occupy a value slot.
                    // Charge every node so a wide array cannot evade byte limits.
                    add_bytes(&mut total, 8, limits.max_event_bytes)?;
                    match value {
                        LogValue::Float(number) if !number.is_finite() => {
                            return Err(LogError::NonFiniteValue);
                        }
                        LogValue::Text(text) => {
                            if text.len() > limits.max_string_bytes {
                                return Err(LogError::ResourceLimit);
                            }
                            add_bytes(&mut total, text.len(), limits.max_event_bytes)?;
                        }
                        LogValue::Bytes(bytes) => {
                            add_bytes(&mut total, bytes.len(), limits.max_event_bytes)?
                        }
                        LogValue::Array(values) => {
                            if depth >= limits.max_depth {
                                return Err(LogError::ResourceLimit);
                            }
                            stack.push(Frame::Array(values.iter(), depth + 1));
                        }
                        LogValue::Object(object) => {
                            if depth >= limits.max_depth {
                                return Err(LogError::ResourceLimit);
                            }
                            stack.push(Frame::Object(object.0.iter(), depth + 1));
                        }
                        _ => {}
                    }
                }
                Frame::Array(mut values, depth) => {
                    if let Some(value) = values.next() {
                        stack.push(Frame::Array(values, depth));
                        stack.push(Frame::Value(value, depth));
                    }
                }
                Frame::Object(mut entries, depth) => {
                    if let Some((key, value)) = entries.next() {
                        fields = fields.checked_add(1).ok_or(LogError::ResourceLimit)?;
                        validate_key(key, limits)?;
                        add_bytes(&mut total, key.len(), limits.max_event_bytes)?;
                        stack.push(Frame::Object(entries, depth));
                        stack.push(Frame::Value(value, depth));
                    }
                }
            }
            if fields > limits.max_fields {
                return Err(LogError::ResourceLimit);
            }
        }
    }
    Ok(total)
}
fn validate_event_parts(
    target: &str,
    message: &str,
    fields: &Fields,
    limits: LogLimits,
) -> Result<(), LogError> {
    if target.len() > limits.max_string_bytes || message.len() > limits.max_string_bytes {
        return Err(LogError::ResourceLimit);
    }
    let mut bytes = validate_values(
        fields.0.iter().map(|(key, value)| (key.as_ref(), value)),
        limits,
    )?;
    add_bytes(&mut bytes, target.len(), limits.max_event_bytes)?;
    add_bytes(&mut bytes, message.len(), limits.max_event_bytes)
}

struct Output {
    text: String,
    maximum: usize,
}
impl Output {
    fn push(&mut self, text: &str) -> Result<(), LogError> {
        let size = self
            .text
            .len()
            .checked_add(text.len())
            .ok_or(LogError::ResourceLimit)?;
        if size > self.maximum {
            return Err(LogError::ResourceLimit);
        }
        self.text
            .try_reserve(text.len())
            .map_err(|_| LogError::ResourceLimit)?;
        self.text.push_str(text);
        Ok(())
    }
    fn string(&mut self, text: &str, quoted: bool) -> Result<(), LogError> {
        if quoted {
            self.push("\"")?;
        }
        for ch in text.chars() {
            match ch {
                '\\' => self.push("\\\\")?,
                '"' => self.push("\\\"")?,
                '\n' => self.push("\\n")?,
                '\r' => self.push("\\r")?,
                '\t' => self.push("\\t")?,
                '\u{8}' => self.push("\\b")?,
                '\u{c}' => self.push("\\f")?,
                ch if ch.is_control() => {
                    let mut digits = [0u8; 6];
                    digits[0] = b'\\';
                    digits[1] = b'u';
                    for (index, digit) in digits[2..].iter_mut().enumerate() {
                        *digit = b"0123456789abcdef"[((ch as usize) >> ((3 - index) * 4)) & 15];
                    }
                    self.push(std::str::from_utf8(&digits).map_err(|_| LogError::Host)?)?;
                }
                ch => self.push(ch.encode_utf8(&mut [0u8; 4]))?,
            }
        }
        if quoted {
            self.push("\"")?;
        }
        Ok(())
    }
}

enum FormatFrame<'a> {
    Value(&'a LogValue),
    Array(std::slice::Iter<'a, LogValue>, bool),
    Object {
        entries: Vec<&'a (Arc<str>, LogValue)>,
        index: usize,
    },
}
fn object_frame(fields: &Fields, format: LogFormat) -> FormatFrame<'_> {
    let mut entries: Vec<_> = fields.0.iter().collect();
    if format == LogFormat::JsonLines {
        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    }
    FormatFrame::Object { entries, index: 0 }
}
fn format_values(
    output: &mut Output,
    root: FormatFrame<'_>,
    format: LogFormat,
) -> Result<(), LogError> {
    let mut stack = vec![root];
    while let Some(frame) = stack.pop() {
        match frame {
            FormatFrame::Object { entries, index } => {
                if index == 0 {
                    output.push("{")?;
                }
                if let Some((key, value)) = entries.get(index) {
                    if index != 0 {
                        output.push(",")?;
                    }
                    output.string(key, true)?;
                    output.push(":")?;
                    stack.push(FormatFrame::Object {
                        entries,
                        index: index + 1,
                    });
                    stack.push(FormatFrame::Value(value));
                } else {
                    output.push("}")?;
                }
            }
            FormatFrame::Array(mut values, first) => {
                if let Some(value) = values.next() {
                    if !first {
                        output.push(",")?;
                    }
                    stack.push(FormatFrame::Array(values, false));
                    stack.push(FormatFrame::Value(value));
                } else {
                    output.push("]")?;
                }
            }
            FormatFrame::Value(value) => match value {
                LogValue::Null => output.push("null")?,
                LogValue::Bool(value) => output.push(if *value { "true" } else { "false" })?,
                LogValue::Int(value) => output.push(&value.to_string())?,
                LogValue::UInt(value) => output.push(&value.to_string())?,
                LogValue::Float(value) => output.push(&value.to_string())?,
                LogValue::Text(text) => output.string(text, true)?,
                LogValue::Redacted => {
                    if format == LogFormat::JsonLines {
                        output.push("{\"$redacted\":true}")?;
                    } else {
                        output.push("[REDACTED]")?;
                    }
                }
                LogValue::Bytes(bytes) => {
                    let encoded_size = bytes
                        .len()
                        .checked_add(2)
                        .and_then(|v| v.checked_div(3))
                        .and_then(|v| v.checked_mul(4))
                        .ok_or(LogError::ResourceLimit)?;
                    if encoded_size > output.maximum.saturating_sub(output.text.len()) {
                        return Err(LogError::ResourceLimit);
                    }
                    let encoded = Base64Options::standard(EncodingLimits {
                        max_input_bytes: bytes.len(),
                        max_output_bytes: encoded_size,
                    })
                    .encode(&Bytes::from_slice(bytes))
                    .map_err(|_| LogError::ResourceLimit)?;
                    if format == LogFormat::JsonLines {
                        output.push("{\"$bytes\":\"")?;
                    } else {
                        output.push("b64:")?;
                    }
                    output.push(
                        std::str::from_utf8(encoded.as_slice()).map_err(|_| LogError::Host)?,
                    )?;
                    if format == LogFormat::JsonLines {
                        output.push("\"}")?;
                    }
                }
                LogValue::Array(values) => {
                    output.push("[")?;
                    stack.push(FormatFrame::Array(values.iter(), true));
                }
                LogValue::Object(fields) => {
                    stack.push(object_frame(fields, format));
                }
            },
        }
    }
    Ok(())
}
/// Entire record creation succeeds before the caller invokes any writer.
pub fn format_record(
    event: &LogEvent,
    format: LogFormat,
    limits: LogLimits,
) -> Result<Vec<u8>, LogError> {
    validate_event_parts(event.target(), event.message(), event.fields(), limits)?;
    let mut output = Output {
        text: String::new(),
        maximum: limits.max_event_bytes,
    };
    match format {
        LogFormat::JsonLines => {
            output.push("{\"schema\":\"tondo-log-event-0.1/1\",\"level\":")?;
            output.string(event.level.text(), true)?;
            output.push(",\"target\":")?;
            output.string(event.target(), true)?;
            output.push(",\"message\":")?;
            output.string(event.message(), true)?;
            output.push(",\"time\":")?;
            if let Some(time) = event.timestamp {
                output.string(&time.format(), true)?;
            } else {
                output.push("null")?;
            }
            output.push(",\"fields\":")?;
            format_values(&mut output, object_frame(event.fields(), format), format)?;
            output.push("}\n")?;
        }
        LogFormat::Text => {
            output.push(event.level.text())?;
            output.push(" ")?;
            output.string(event.target(), true)?;
            output.push(" ")?;
            output.string(event.message(), true)?;
            for (key, value) in event.fields.0.iter() {
                output.push(" ")?;
                output.string(key, true)?;
                output.push("=")?;
                format_values(&mut output, FormatFrame::Value(value), format)?;
            }
            output.push("\n")?;
        }
    }
    Ok(output.text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logging_registry_has_only_two_private_pure_construction_helpers() {
        assert_eq!(LogOperation::ALL.len(), 2);
        let mut names = std::collections::BTreeSet::new();
        for operation in LogOperation::ALL {
            assert!(names.insert(operation.name()));
            assert_eq!(LogOperation::from_name(operation.name()), Some(operation));
            let (owner, member) = operation
                .name()
                .strip_prefix("std.log.")
                .unwrap()
                .split_once('.')
                .unwrap();
            assert!(member.starts_with("__"));
            assert_eq!(LogOperation::associated(owner, member), Some(operation));
        }
        for name in [
            "std.log.Fields.put",
            "std.log.LogEvent.create",
            "std.log.LogEvent.level",
            "std.log.Logger.emit",
            "std.log.ConsoleSink.create",
            "std.log.FileSink.create",
        ] {
            assert_eq!(LogOperation::from_name(name), None);
        }
    }
    fn sample(fields: Fields) -> LogEvent {
        LogEvent::create(
            LogLevel::Info,
            "server".into(),
            "hello\n世界".into(),
            fields,
            None,
        )
        .unwrap()
    }
    #[test]
    fn logging_fields_are_atomic_and_copies_independent() {
        let mut fields = Fields::empty();
        fields.put("x".into(), LogValue::Int(3)).unwrap();
        let previous = fields.clone();
        assert_eq!(
            fields.put("x".into(), LogValue::Null),
            Err(LogError::DuplicateField)
        );
        assert_eq!(
            fields.put("bad\0".into(), LogValue::Null),
            Err(LogError::InvalidFieldKey)
        );
        assert_eq!(
            fields.put("bad".into(), LogValue::Float(f64::NAN)),
            Err(LogError::NonFiniteValue)
        );
        assert_eq!(fields, previous);
        fields.put("y".into(), LogValue::Null).unwrap();
        assert_eq!(previous.count(), 1);
        assert_eq!(fields.count(), 2);
    }
    #[test]
    fn logging_json_is_exact_canonical_and_single_line() {
        let mut fields = Fields::empty();
        fields.put("z".into(), LogValue::Redacted).unwrap();
        fields
            .put("a".into(), LogValue::Bytes(Arc::from([0, 255])))
            .unwrap();
        let result =
            format_record(&sample(fields), LogFormat::JsonLines, LogLimits::default()).unwrap();
        assert_eq!(
            String::from_utf8(result).unwrap(),
            "{\"schema\":\"tondo-log-event-0.1/1\",\"level\":\"info\",\"target\":\"server\",\"message\":\"hello\\n世界\",\"time\":null,\"fields\":{\"a\":{\"$bytes\":\"AP8=\"},\"z\":{\"$redacted\":true}}}\n"
        );
    }
    #[test]
    fn logging_text_preserves_field_order_and_quotes_controls() {
        let mut fields = Fields::empty();
        fields.put("z".into(), LogValue::Redacted).unwrap();
        fields.put("a".into(), LogValue::Bool(true)).unwrap();
        let result = format_record(&sample(fields), LogFormat::Text, LogLimits::default()).unwrap();
        assert_eq!(
            String::from_utf8(result).unwrap(),
            "info \"server\" \"hello\\n世界\" \"z\"=[REDACTED] \"a\"=true\n"
        );
    }
    #[test]
    fn logging_nested_depth_and_formatted_expansion_are_bounded() {
        let mut value = LogValue::Null;
        for _ in 0..17 {
            value = LogValue::Array(Arc::from([value]));
        }
        let mut fields = Fields::empty();
        assert_eq!(
            fields.put("nested".into(), value),
            Err(LogError::ResourceLimit)
        );
        assert_eq!(fields.count(), 0);
        let event = sample(fields);
        let limits = LogLimits {
            max_event_bytes: 30,
            ..LogLimits::default()
        };
        assert_eq!(
            format_record(&event, LogFormat::JsonLines, limits),
            Err(LogError::ResourceLimit)
        );
    }
    #[test]
    fn logging_filter_and_timestamp_do_not_read_providers() {
        let options = LoggerOptions::create(LogLevel::Warn);
        assert!(!options.enabled(LogLevel::Info));
        assert!(options.enabled(LogLevel::Warn));
        let event = LogEvent::create(
            LogLevel::Trace,
            "x".into(),
            "".into(),
            Fields::empty(),
            Some(UtcDateTime::parse("2024-02-29T23:59:59.000000001Z").unwrap()),
        )
        .unwrap();
        let output = format_record(&event, LogFormat::JsonLines, LogLimits::default()).unwrap();
        assert!(
            std::str::from_utf8(&output)
                .unwrap()
                .contains("\"time\":\"2024-02-29T23:59:59.000000001Z\"")
        );
    }
}

#[cfg(test)]
mod edge_tests {
    use super::*;

    #[test]
    fn field_key_limits_count_utf8_bytes_without_normalization() {
        let mut fields = Fields::empty();
        fields.put("é".repeat(64), LogValue::Null).unwrap();
        let before = fields.clone();
        assert_eq!(
            fields.put("é".repeat(65), LogValue::Null),
            Err(LogError::ResourceLimit)
        );
        assert_eq!(fields, before);
        fields.put("e\u{301}".into(), LogValue::Null).unwrap();
        fields.put("é".into(), LogValue::Null).unwrap();
        assert_eq!(fields.count(), 3);
    }

    #[test]
    fn field_count_limit_accepts_the_boundary_and_preserves_it_on_rejection() {
        let mut fields = Fields::empty();
        for index in 0..64 {
            fields.put(format!("field{index}"), LogValue::Null).unwrap();
        }
        let before = fields.clone();
        assert_eq!(
            fields.put("overflow".into(), LogValue::Bool(true)),
            Err(LogError::ResourceLimit)
        );
        assert_eq!(fields, before);
    }

    #[test]
    fn nested_values_accept_the_exact_depth_boundary() {
        let mut value = LogValue::Null;
        for _ in 0..16 {
            value = LogValue::Array(Arc::from([value]));
        }
        let mut fields = Fields::empty();
        fields.put("nested".into(), value).unwrap();
        let event =
            LogEvent::create(LogLevel::Info, "app".into(), "".into(), fields, None).unwrap();
        let line = format_record(&event, LogFormat::JsonLines, LogLimits::default()).unwrap();
        assert_eq!(line.iter().filter(|byte| **byte == b'[').count(), 16);
    }

    #[test]
    fn events_keep_their_field_snapshot_after_builder_mutation() {
        let mut fields = Fields::empty();
        fields.put("before".into(), LogValue::Int(1)).unwrap();
        let event = LogEvent::create(
            LogLevel::Info,
            "app".into(),
            "".into(),
            fields.clone(),
            None,
        )
        .unwrap();
        fields.put("after".into(), LogValue::Int(2)).unwrap();
        assert_eq!(event.fields().count(), 1);
    }

    #[test]
    fn queue_capacity_has_explicit_positive_bounded_admission() {
        let limits = LogLimits::default();
        assert_eq!(
            SinkOptions::create(LogFormat::Text, Backpressure::Block, 0, limits),
            Err(LogError::InvalidLimit)
        );
        assert_eq!(
            SinkOptions::create(
                LogFormat::Text,
                Backpressure::Reject,
                limits.max_queue_entries + 1,
                limits,
            ),
            Err(LogError::ResourceLimit)
        );
        assert!(
            SinkOptions::create(
                LogFormat::JsonLines,
                Backpressure::Drop,
                limits.max_queue_entries,
                limits,
            )
            .is_ok()
        );
    }

    #[test]
    fn empty_scalar_arrays_cannot_evade_the_logical_event_budget() {
        let mut fields = Fields::empty();
        let values = vec![LogValue::Text(Arc::from("")); (1 << 20) / 8 + 1];
        assert_eq!(
            fields.put("empty".into(), LogValue::Array(values.into())),
            Err(LogError::ResourceLimit)
        );
        assert_eq!(fields.count(), 0);
    }
}
